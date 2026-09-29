//! Editor detection, command templates and launching (T3.16, design §11.13,
//! ADR-0021, OQ-21). Nothing here starts a real editor: shims, bundles and
//! editors are fake executables in temp dirs, and launches go through a
//! recording [`Spawner`] (or, where a real process proves "no shell", a fake
//! editor script that records its arguments).

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use polygloss_platform::editor::{
    DetectEnv, EditorCommand, ProcessSpawner, Spawner, detect_in, shell_quote,
};

/// Plan "Test hygiene": a temp `HOME`, config and cache dir and an empty
/// global git config for this test process (nextest runs one test per
/// process), kept alive by the returned dir.
fn sandbox() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp dir");
    let home = root.path().join("home");
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::fs::write(home.join(".gitconfig-empty"), "").unwrap();
    // SAFETY: first thing the test does, before other threads start.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("POLYGLOSS_DATA_DIR", root.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
        std::env::set_var("GIT_CONFIG_GLOBAL", home.join(".gitconfig-empty"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    root
}

/// Records every argv instead of starting anything.
#[derive(Default)]
struct Recorder(Mutex<Vec<Vec<OsString>>>);

impl Spawner for Recorder {
    fn spawn(&self, argv: &[OsString]) -> io::Result<()> {
        self.0.lock().unwrap().push(argv.to_vec());
        Ok(())
    }
}

impl Recorder {
    fn calls(&self) -> Vec<Vec<OsString>> {
        self.0.lock().unwrap().clone()
    }
}

fn os(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

/// An executable file at `path` running `script` with `/bin/sh`.
fn executable(path: &Path, script: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake editor at `path` that writes its arguments to `out`, each
/// NUL-terminated (atomically: written aside, then moved into place).
fn recording_editor(path: &Path, out: &Path) {
    let quoted = String::from_utf8(shell_quote(out.as_os_str())).unwrap();
    let script = format!(
        "for a in \"$@\"; do printf '%s\\0' \"$a\"; done > {quoted}.tmp && mv {quoted}.tmp {quoted}"
    );
    executable(path, &script);
}

/// Waits (≤ 10 s) for `out` and returns the arguments it records.
fn recorded_args(out: &Path) -> Vec<OsString> {
    let start = Instant::now();
    loop {
        if let Ok(bytes) = std::fs::read(out) {
            return bytes
                .split(|&b| b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| OsString::from_vec(a.to_vec()))
                .collect();
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the fake editor never ran"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn template_expands_path_and_line_without_shell() {
    let root = sandbox();
    let hostile = Path::new("/tmp/a b/$(touch pwned); `x` 'q' \"d\".rs");

    // `{path}` and `{line}` are substituted inside one argument; nothing
    // is interpreted.
    let zed = EditorCommand::from_template("zed {path}:{line}");
    assert_eq!(
        zed.argv(hostile, 12),
        os(&["zed", "/tmp/a b/$(touch pwned); `x` 'q' \"d\".rs:12"])
    );
    assert!(!zed.is_terminal());

    // Quotes group words (a program path with spaces); a template without
    // `{path}` gets the path appended.
    let quoted = EditorCommand::from_template(
        r#""/Applications/My Editor.app/bin/ed" --goto '{path}:{line}'"#,
    );
    assert_eq!(
        quoted.argv(Path::new("/src/lib.rs"), 3),
        os(&[
            "/Applications/My Editor.app/bin/ed",
            "--goto",
            "/src/lib.rs:3"
        ])
    );
    assert_eq!(
        EditorCommand::from_template("subl").argv(Path::new("/src/lib.rs"), 3),
        os(&["subl", "/src/lib.rs"])
    );
    assert_eq!(
        EditorCommand::from_template(r"my\ editor -l {line} {path}").argv(Path::new("/x"), 9),
        os(&["my editor", "-l", "9", "/x"])
    );

    // Non-UTF-8 path bytes pass through untouched.
    let raw = PathBuf::from(OsString::from_vec(b"/tmp/caf\xe9.txt".to_vec()));
    let argv = zed.argv(&raw, 1);
    assert_eq!(argv[1].as_bytes(), b"/tmp/caf\xe9.txt:1");

    // Launching hands exactly that argv to the spawner: no `sh -c`.
    let recorder = Recorder::default();
    zed.launch(hostile, 12, &recorder, root.path()).unwrap();
    assert_eq!(recorder.calls(), vec![zed.argv(hostile, 12)]);

    // A real process gets the hostile path as one argument, and nothing in
    // it runs.
    let work = root.path().join("work");
    let out = root.path().join("args");
    let editor = root.path().join("bin/fake-editor");
    recording_editor(&editor, &out);
    let template = format!(
        "{} --line {{line}} {{path}}",
        String::from_utf8(shell_quote(editor.as_os_str())).unwrap()
    );
    std::fs::create_dir_all(&work).unwrap();
    let path = work.join("$(touch pwned); `touch pwned2` it's.rs");
    EditorCommand::from_template(&template)
        .launch(&path, 4, &ProcessSpawner, root.path())
        .unwrap();
    let args = recorded_args(&out);
    assert_eq!(
        args,
        vec![
            OsString::from("--line"),
            OsString::from("4"),
            path.clone().into_os_string()
        ]
    );
    assert!(!work.join("pwned").exists());
    assert!(!Path::new("pwned").exists());
    assert!(!work.join("pwned2").exists());
}

#[test]
fn detect_order_zed_cursor_vscode_then_env() {
    let root = sandbox();
    let bin = root.path().join("bin");
    let apps = root.path().join("Applications");
    std::fs::create_dir_all(&apps).unwrap();
    for shim in ["zed", "cursor", "code"] {
        executable(&bin.join(shim), "exit 0");
    }
    // Not executable: never picked.
    std::fs::write(bin.join("vim"), "").unwrap();
    let mut env = DetectEnv {
        search_path: vec![root.path().join("missing"), bin.clone()],
        app_dirs: vec![apps.clone()],
        visual: None,
        editor: None,
    };
    let file = Path::new("/r/src/lib.rs");
    let argv = |env: &DetectEnv| detect_in(env).map(|c| c.argv(file, 3));
    let path_str = |p: PathBuf| p.into_os_string();

    // Zed first: its CLI shim, else the CLI inside the app bundle.
    assert_eq!(
        argv(&env),
        Some(vec![path_str(bin.join("zed")), "/r/src/lib.rs:3".into()])
    );
    std::fs::remove_file(bin.join("zed")).unwrap();
    let zed_cli = apps.join("Zed.app/Contents/MacOS/cli");
    executable(&zed_cli, "exit 0");
    assert_eq!(
        argv(&env),
        Some(vec![path_str(zed_cli.clone()), "/r/src/lib.rs:3".into()])
    );
    std::fs::remove_dir_all(apps.join("Zed.app")).unwrap();

    // Then Cursor, then VS Code (shim or bundle), both with `-g`.
    assert_eq!(
        argv(&env),
        Some(vec![
            path_str(bin.join("cursor")),
            "-g".into(),
            "/r/src/lib.rs:3".into()
        ])
    );
    std::fs::remove_file(bin.join("cursor")).unwrap();
    std::fs::remove_file(bin.join("code")).unwrap();
    let code_cli = apps.join("Visual Studio Code.app/Contents/Resources/app/bin/code");
    executable(&code_cli, "exit 0");
    assert_eq!(
        argv(&env),
        Some(vec![
            path_str(code_cli.clone()),
            "-g".into(),
            "/r/src/lib.rs:3".into()
        ])
    );
    std::fs::remove_dir_all(apps.join("Visual Studio Code.app")).unwrap();

    // Then `$VISUAL` (resolved on the search path, with its own flags and
    // the editor's line syntax), then `$EDITOR`.
    executable(&bin.join("nvim"), "exit 0");
    env.visual = Some("nvim -p".into());
    env.editor = Some("nano".into());
    let visual = detect_in(&env).expect("$VISUAL");
    assert!(visual.is_terminal());
    assert_eq!(
        visual.argv(file, 3),
        vec![
            path_str(bin.join("nvim")),
            "-p".into(),
            "+3".into(),
            "/r/src/lib.rs".into()
        ]
    );
    env.visual = Some("".into());
    let editor = detect_in(&env).expect("$EDITOR");
    assert!(editor.is_terminal());
    assert_eq!(editor.argv(file, 3), os(&["nano", "+3", "/r/src/lib.rs"]));
    // A GUI editor named by `$EDITOR` keeps its line syntax too.
    env.editor = Some("code --wait".into());
    assert_eq!(
        argv(&env),
        Some(os(&["code", "--wait", "-g", "/r/src/lib.rs:3"]))
    );
    env.editor = Some("mystery-editor".into());
    assert_eq!(argv(&env), Some(os(&["mystery-editor", "/r/src/lib.rs"])));

    env.editor = None;
    assert!(detect_in(&env).is_none());
}

#[test]
fn terminal_editor_uses_command_file() {
    let root = sandbox();
    let scratch = root.path().join("cache/commands");
    let out = root.path().join("args");
    let vim = root.path().join("bin/vim");
    recording_editor(&vim, &out);
    let template = format!(
        "{} +{{line}} {{path}}",
        String::from_utf8(shell_quote(vim.as_os_str())).unwrap()
    );
    let cmd = EditorCommand::from_template(&template);
    assert!(cmd.is_terminal(), "vim runs in a terminal");

    let path = PathBuf::from(OsString::from_vec(
        b"/tmp/it's a \"path\" $(touch pwned) \xe9.rs".to_vec(),
    ));
    let recorder = Recorder::default();
    cmd.launch(&path, 7, &recorder, &scratch).unwrap();

    // The default terminal app opens a temporary `.command` file.
    let calls = recorder.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].len(), 2, "{calls:?}");
    assert_eq!(calls[0][0], OsStr::new("/usr/bin/open"));
    let file = PathBuf::from(&calls[0][1]);
    assert_eq!(file.parent(), Some(scratch.as_path()));
    assert_eq!(file.extension(), Some(OsStr::new("command")));
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "{mode:o}");

    // Running it (as Terminal would) starts the editor with the exact
    // arguments, and the file removes itself.
    let status = std::process::Command::new("/bin/sh")
        .arg(&file)
        .current_dir(root.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        recorded_args(&out),
        vec![OsString::from("+7"), path.into_os_string()]
    );
    assert!(!file.exists(), "the .command file removes itself");
    assert!(!root.path().join("pwned").exists());

    // GUI editors never go through a command file.
    let recorder = Recorder::default();
    EditorCommand::from_template("zed {path}:{line}")
        .launch(Path::new("/x.rs"), 1, &recorder, &scratch)
        .unwrap();
    assert_eq!(recorder.calls(), vec![os(&["zed", "/x.rs:1"])]);
}

#[test]
fn template_program_resolves_on_search_path() {
    let root = sandbox();
    let bin = root.path().join("bin");
    executable(&bin.join("zed"), "exit 0");
    let dirs = vec![root.path().join("missing"), bin.clone()];
    // An app started from the Finder has a minimal `PATH`: a bare program
    // name is looked up in the shim directories.
    let zed = EditorCommand::from_template("zed {path}:{line}").resolved(&dirs);
    assert_eq!(
        zed.argv(Path::new("/x.rs"), 2),
        vec![bin.join("zed").into_os_string(), "/x.rs:2".into()]
    );
    // Paths and unknown programs stay as written.
    let abs = EditorCommand::from_template("/opt/ed {path}").resolved(&dirs);
    assert_eq!(abs.program(), Some(OsStr::new("/opt/ed")));
    let unknown = EditorCommand::from_template("nope {path}").resolved(&dirs);
    assert_eq!(unknown.program(), Some(OsStr::new("nope")));
}
