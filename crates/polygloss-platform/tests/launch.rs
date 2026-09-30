//! The launch helper (T4.2, design §13.4): probe the app socket, else launch
//! the app (`open -g -b dev.dak.polygloss [url]`, or `$POLYGLOSS_APP_BIN` in
//! tests) and wait for its socket.
//!
//! Every test builds its own [`DataPaths`] under a temp dir (never the real
//! `~/Library`), and launches through a recording [`Launcher`] or a shell
//! script standing in for the app, so no real app is ever started.

use std::ffi::OsString;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use polygloss_core::paths::DataPaths;
use polygloss_platform::launch::{
    self, BUNDLE_ID, LaunchOutcome, Launcher, SystemLauncher, app_socket_path, ensure_app,
    ensure_app_within,
};

/// Paths under `root` (its own `HOME` and data dir).
fn paths_in(root: &Path) -> DataPaths {
    let home = root.join("home");
    let data = root.join("data");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    DataPaths::resolve_with(|k| match k {
        "HOME" => Some(home.clone().into_os_string()),
        "POLYGLOSS_DATA_DIR" => Some(data.clone().into_os_string()),
        _ => None,
    })
    .unwrap()
}

/// A short temp dir, so the socket path fits `sun_path`.
fn short_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("pg")
        .tempdir_in("/tmp")
        .unwrap()
}

type Calls = Arc<Mutex<Vec<(Option<String>, bool)>>>;

/// Records every launch and runs `then` (e.g. binds the socket later).
struct Recording {
    calls: Calls,
    then: Box<dyn Fn() -> std::io::Result<()>>,
}

impl Recording {
    fn new(then: impl Fn() -> std::io::Result<()> + 'static) -> (Recording, Calls) {
        let calls = Calls::default();
        (
            Recording {
                calls: calls.clone(),
                then: Box::new(then),
            },
            calls,
        )
    }
}

impl Launcher for Recording {
    fn launch(&self, url: Option<&str>, activate: bool) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((url.map(str::to_owned), activate));
        (self.then)()
    }
}

/// Binds `socket` after `delay` on a thread and keeps listening until the
/// test process ends.
fn bind_later(socket: PathBuf, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let listener = UnixListener::bind(&socket).unwrap();
        for stream in listener.incoming() {
            drop(stream);
        }
    });
}

#[test]
fn ensure_app_already_running_skips_launch() {
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    let _app = UnixListener::bind(app_socket_path(&paths)).unwrap();
    let (launcher, calls) = Recording::new(|| Ok(()));

    let outcome = ensure_app(&paths, Some("polygloss://review/x"), true, &launcher);

    assert_eq!(outcome, LaunchOutcome::AlreadyRunning);
    assert!(calls.lock().unwrap().is_empty(), "no launch");
    assert!(launch::app_is_running(&paths));
}

#[test]
fn ensure_app_launches_and_waits_for_socket() {
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    let socket = app_socket_path(&paths);
    let (launcher, calls) = Recording::new(move || {
        bind_later(socket.clone(), Duration::from_millis(300));
        Ok(())
    });

    let start = Instant::now();
    let outcome = ensure_app(&paths, Some("polygloss://diff/abc"), false, &launcher);

    assert_eq!(outcome, LaunchOutcome::Launched);
    let waited = start.elapsed();
    assert!(waited >= Duration::from_millis(300), "{waited:?}");
    assert!(waited < Duration::from_secs(5), "{waited:?}");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![(Some("polygloss://diff/abc".to_owned()), false)]
    );
    assert!(launch::app_is_running(&paths));
}

#[test]
fn ensure_app_treats_stale_socket_as_not_running() {
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    let socket = app_socket_path(&paths);
    // A crashed app leaves its socket file behind: nothing listens on it.
    drop(UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    assert!(!launch::app_is_running(&paths));

    let s = socket.clone();
    let (launcher, calls) = Recording::new(move || {
        // The relaunched app replaces the stale file (T4.1 `serve`).
        std::fs::remove_file(&s)?;
        bind_later(s.clone(), Duration::from_millis(50));
        Ok(())
    });
    let outcome = ensure_app(&paths, None, true, &launcher);
    assert_eq!(outcome, LaunchOutcome::Launched);
    assert_eq!(*calls.lock().unwrap(), vec![(None, true)]);
}

#[test]
fn ensure_app_times_out_as_unavailable() {
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    let (launcher, calls) = Recording::new(|| Ok(()));

    let start = Instant::now();
    let outcome = ensure_app_within(&paths, None, false, &launcher, Duration::from_millis(400));

    let waited = start.elapsed();
    assert!(
        matches!(&outcome, LaunchOutcome::Unavailable(m) if m.contains("did not start")),
        "{outcome:?}"
    );
    assert!(waited >= Duration::from_millis(400), "{waited:?}");
    assert!(waited < Duration::from_secs(3), "{waited:?}");
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(launch::LAUNCH_TIMEOUT, Duration::from_secs(10));
}

#[test]
fn ensure_app_reports_launch_failure_as_unavailable() {
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    let (launcher, _calls) = Recording::new(|| {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Unable to find application",
        ))
    });
    let start = Instant::now();
    let outcome = ensure_app(&paths, None, false, &launcher);
    assert!(
        matches!(&outcome, LaunchOutcome::Unavailable(m) if m.contains("Unable to find application")),
        "{outcome:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "no wait after a failed launch"
    );
}

#[test]
fn activate_false_uses_background_flag() {
    let open = SystemLauncher::Open;
    let argv = |url: Option<&str>, activate: bool| -> Vec<String> {
        open.argv(url, activate)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect()
    };
    assert_eq!(
        argv(Some("polygloss://review/r"), false),
        [
            "/usr/bin/open",
            "-g",
            "-b",
            BUNDLE_ID,
            "polygloss://review/r"
        ]
    );
    assert_eq!(argv(None, false), ["/usr/bin/open", "-g", "-b", BUNDLE_ID]);
    assert_eq!(
        argv(Some("polygloss://review/r"), true),
        ["/usr/bin/open", "-b", BUNDLE_ID, "polygloss://review/r"]
    );
    assert_eq!(BUNDLE_ID, "dev.dak.polygloss");

    // The dev/test binary gets the URL as its argument, whatever `activate`.
    let bin = SystemLauncher::AppBin(PathBuf::from("/x/Polygloss"));
    assert_eq!(
        bin.argv(Some("polygloss://review/r"), false),
        [
            OsString::from("/x/Polygloss"),
            "polygloss://review/r".into()
        ]
    );
    assert_eq!(bin.argv(None, true), [OsString::from("/x/Polygloss")]);
}

#[test]
fn app_bin_override_only_in_test_mode() {
    let env = |test: Option<&str>, bin: Option<&str>| {
        SystemLauncher::from_env_with(|k| match k {
            "POLYGLOSS_TEST" => test.map(OsString::from),
            "POLYGLOSS_APP_BIN" => bin.map(OsString::from),
            _ => None,
        })
    };
    assert_eq!(
        env(Some("1"), Some("/x/Polygloss")),
        SystemLauncher::AppBin(PathBuf::from("/x/Polygloss"))
    );
    assert_eq!(env(Some("1"), None), SystemLauncher::Open);
    assert_eq!(env(Some("1"), Some("")), SystemLauncher::Open);
    assert_eq!(env(None, None), SystemLauncher::Open);
}

#[test]
fn app_bin_without_test_mode_refuses_to_launch() {
    // `POLYGLOSS_APP_BIN` outside test mode is a misconfigured test or gate
    // run: launching the installed bundle instead would start the real app
    // on the real data dir, so nothing launches and the reason is reported.
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    for test in [None, Some("0")] {
        let launcher = SystemLauncher::from_env_with(|k| match k {
            "POLYGLOSS_TEST" => test.map(OsString::from),
            "POLYGLOSS_APP_BIN" => Some(OsString::from("/x/Polygloss")),
            _ => None,
        });
        assert!(
            matches!(&launcher, SystemLauncher::Refused(m) if m.contains("POLYGLOSS_TEST=1")),
            "{launcher:?}"
        );
        assert!(
            launcher
                .argv(Some("polygloss://review/r"), false)
                .is_empty()
        );
        let err = launcher.launch(None, false).unwrap_err();
        assert!(err.to_string().contains("POLYGLOSS_APP_BIN"), "{err}");

        let start = Instant::now();
        let outcome = ensure_app(&paths, None, false, &launcher);
        assert!(
            matches!(&outcome, LaunchOutcome::Unavailable(m) if m.contains("POLYGLOSS_TEST=1")),
            "{outcome:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(2), "no wait");
    }
}

#[test]
fn app_bin_launcher_spawns_detached_with_url() {
    let dir = short_tempdir();
    let out = dir.path().join("argv.txt");
    let script = dir.path().join("fake-app.sh");
    // Sleeps first: `launch` must return without waiting for the app to exit.
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nsleep 0.3\nprintf '%s\\n' \"$@\" > '{}.tmp' && mv '{0}.tmp' '{0}'\n",
            out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let start = Instant::now();
    SystemLauncher::AppBin(script)
        .launch(Some("polygloss://thread/t"), false)
        .unwrap();
    assert!(start.elapsed() < Duration::from_millis(250), "detached");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !out.exists() {
        assert!(Instant::now() < deadline, "the fake app never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "polygloss://thread/t\n"
    );
}

#[test]
fn app_socket_path_falls_back_to_tmpdir_when_too_long() {
    let dir = short_tempdir();
    let paths = paths_in(dir.path());
    assert_eq!(app_socket_path(&paths), paths.socket);

    let long = dir.path().join("d".repeat(120));
    let long_paths = DataPaths::resolve_with(|k| match k {
        "HOME" => Some(dir.path().as_os_str().to_owned()),
        "POLYGLOSS_DATA_DIR" => Some(long.clone().into_os_string()),
        _ => None,
    })
    .unwrap();
    let socket = app_socket_path(&long_paths);
    assert_ne!(socket, long_paths.socket);
    assert!(socket.as_os_str().len() <= polygloss_core::paths::SOCKET_PATH_MAX);
    assert_eq!(socket.file_name().unwrap(), "polygloss.sock");
    let parent = socket.parent().unwrap();
    assert!(parent.starts_with(std::env::temp_dir()));
    let name = parent.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("polygloss-"), "{name}");
    assert!(name["polygloss-".len()..].parse::<u32>().is_ok(), "{name}");
}
