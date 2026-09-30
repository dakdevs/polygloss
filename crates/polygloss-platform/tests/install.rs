//! The stable CLI path `<data_dir>/bin/polygloss` (design §13.2, T4.3): the
//! app points it at its own `Contents/MacOS/polygloss-cli` at every launch.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use polygloss_platform::install::{
    CLI_LINK_NAME, SymlinkRefresh, bundled_cli, refresh_for_exe, refresh_stable_symlink,
};

fn touch_exe(path: &Path) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, "#!/bin/sh\n").expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn link_target(bin_dir: &Path) -> PathBuf {
    fs::read_link(bin_dir.join(CLI_LINK_NAME)).expect("read link")
}

#[test]
fn stable_symlink_refreshed_and_replaces_stale_target() {
    let root = tempfile::tempdir().expect("temp dir");
    let bin_dir = root.path().join("data/bin");
    let old_cli = root.path().join("Old.app/Contents/MacOS/polygloss-cli");
    let new_cli = root.path().join("New.app/Contents/MacOS/polygloss-cli");
    touch_exe(&old_cli);
    touch_exe(&new_cli);

    // First launch: the dir and the link are created.
    assert_eq!(
        refresh_stable_symlink(&bin_dir, &old_cli).expect("create"),
        SymlinkRefresh::Created
    );
    assert_eq!(link_target(&bin_dir), old_cli);

    // Same app again: nothing to do.
    assert_eq!(
        refresh_stable_symlink(&bin_dir, &old_cli).expect("again"),
        SymlinkRefresh::Unchanged
    );

    // The app moved (or a new copy launched): the stale link is replaced.
    assert_eq!(
        refresh_stable_symlink(&bin_dir, &new_cli).expect("replace"),
        SymlinkRefresh::Replaced
    );
    assert_eq!(link_target(&bin_dir), new_cli);

    // A dangling link (the old app was deleted) is replaced too.
    fs::remove_file(&new_cli).expect("rm new");
    assert_eq!(
        refresh_stable_symlink(&bin_dir, &old_cli).expect("dangling"),
        SymlinkRefresh::Replaced
    );
    assert_eq!(link_target(&bin_dir), old_cli);

    // A regular file left at the stable path is replaced by the link.
    let link = bin_dir.join(CLI_LINK_NAME);
    fs::remove_file(&link).expect("rm link");
    fs::write(&link, "stale copy").expect("write stale file");
    assert_eq!(
        refresh_stable_symlink(&bin_dir, &old_cli).expect("file"),
        SymlinkRefresh::Replaced
    );
    assert_eq!(link_target(&bin_dir), old_cli);

    // No temporary entries are left behind.
    let names: Vec<_> = fs::read_dir(&bin_dir)
        .expect("read bin dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from(CLI_LINK_NAME)]);
}

#[test]
fn stable_symlink_never_removes_a_directory() {
    let root = tempfile::tempdir().expect("temp dir");
    let bin_dir = root.path().join("bin");
    let cli = root.path().join("A.app/Contents/MacOS/polygloss-cli");
    touch_exe(&cli);
    fs::create_dir_all(bin_dir.join(CLI_LINK_NAME).join("keep")).expect("mkdir");
    assert!(refresh_stable_symlink(&bin_dir, &cli).is_err());
    assert!(bin_dir.join(CLI_LINK_NAME).join("keep").is_dir());
}

#[test]
fn bundled_cli_only_inside_an_app_bundle() {
    let root = tempfile::tempdir().expect("temp dir");
    let app_exe = root.path().join("Polygloss.app/Contents/MacOS/Polygloss");
    let cli = root
        .path()
        .join("Polygloss.app/Contents/MacOS/polygloss-cli");
    touch_exe(&app_exe);
    // No CLI next to the app yet.
    assert_eq!(bundled_cli(&app_exe), None);
    touch_exe(&cli);
    assert_eq!(bundled_cli(&app_exe), Some(cli.clone()));

    // An unbundled dev build (`target/debug/Polygloss`) is left alone, even
    // with a `polygloss-cli` next to it.
    let dev_exe = root.path().join("target/debug/Polygloss");
    touch_exe(&dev_exe);
    touch_exe(&root.path().join("target/debug/polygloss-cli"));
    assert_eq!(bundled_cli(&dev_exe), None);
    // `Contents/MacOS` outside a `.app` is not a bundle either.
    let odd = root.path().join("Folder/Contents/MacOS/Polygloss");
    touch_exe(&odd);
    touch_exe(&root.path().join("Folder/Contents/MacOS/polygloss-cli"));
    assert_eq!(bundled_cli(&odd), None);
}

#[test]
fn refresh_for_exe_links_bundles_and_skips_dev_builds() {
    let root = tempfile::tempdir().expect("temp dir");
    let bin_dir = root.path().join("data/bin");
    let dev_exe = root.path().join("target/debug/Polygloss");
    touch_exe(&dev_exe);
    touch_exe(&root.path().join("target/debug/polygloss-cli"));
    assert_eq!(refresh_for_exe(&bin_dir, &dev_exe).expect("dev"), None);
    assert!(!bin_dir.exists(), "a dev build must not create the bin dir");

    let app_exe = root.path().join("Polygloss.app/Contents/MacOS/Polygloss");
    let cli = root
        .path()
        .join("Polygloss.app/Contents/MacOS/polygloss-cli");
    touch_exe(&app_exe);
    touch_exe(&cli);
    assert_eq!(
        refresh_for_exe(&bin_dir, &app_exe).expect("bundle"),
        Some(SymlinkRefresh::Created)
    );
    assert_eq!(link_target(&bin_dir), cli);
}

// ---- Install CLI (T5.4): `/usr/local/bin/polygloss` → the bundle's CLI ----
//
// Every test links into a temp dir; the admin fallback goes to a recording
// runner (no password prompt, nothing outside the temp dir is touched).

mod install_cli {
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use polygloss_platform::install::{
        AdminOutput, AdminRunner, CliInstall, INSTALL_LINK, InstallError, admin_link_argv,
        install_cli_at,
    };

    use super::touch_exe;

    /// Fails the test if the admin fallback runs.
    struct NoAdmin;

    impl AdminRunner for NoAdmin {
        fn run(&self, argv: &[OsString]) -> io::Result<AdminOutput> {
            panic!("the admin fallback ran: {argv:?}");
        }
    }

    /// Records the argv; `emulate` plays the admin's part (or not).
    struct FakeAdmin {
        calls: Mutex<Vec<Vec<OsString>>>,
        outcome: Outcome,
    }

    #[derive(Clone, Copy)]
    enum Outcome {
        /// Makes the link as root would (chmods the dir around it).
        Link,
        /// The user pressed Cancel.
        Cancel,
        /// Says it worked but links nothing.
        LieSuccess,
    }

    impl FakeAdmin {
        fn new(outcome: Outcome) -> FakeAdmin {
            FakeAdmin {
                calls: Mutex::new(Vec::new()),
                outcome,
            }
        }

        fn calls(&self) -> Vec<Vec<OsString>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl AdminRunner for FakeAdmin {
        fn run(&self, argv: &[OsString]) -> io::Result<AdminOutput> {
            self.calls.lock().unwrap().push(argv.to_vec());
            match self.outcome {
                Outcome::Link => {
                    // The last three argv items are dir, target and link.
                    let n = argv.len();
                    let dir = Path::new(&argv[n - 3]);
                    let (target, link) = (Path::new(&argv[n - 2]), Path::new(&argv[n - 1]));
                    fs::create_dir_all(dir)?;
                    let was = fs::metadata(dir)?.permissions();
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
                    let _ = fs::remove_file(link);
                    symlink(target, link)?;
                    fs::set_permissions(dir, was)?;
                    Ok(AdminOutput {
                        success: true,
                        stderr: String::new(),
                    })
                }
                Outcome::Cancel => Ok(AdminOutput {
                    success: false,
                    stderr: "0:180: execution error: User canceled. (-128)\n".into(),
                }),
                Outcome::LieSuccess => Ok(AdminOutput {
                    success: true,
                    stderr: String::new(),
                }),
            }
        }
    }

    struct Fixture {
        _root: tempfile::TempDir,
        root: PathBuf,
        target: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().expect("temp dir");
        let path = root.path().canonicalize().expect("canonical temp dir");
        let target = path.join("Polygloss.app/Contents/MacOS/polygloss-cli");
        touch_exe(&target);
        Fixture {
            _root: root,
            root: path,
            target,
        }
    }

    /// Makes `dir` read-only for the rest of the test (and writable again
    /// on drop, so the temp dir can be removed).
    struct ReadOnly(PathBuf);

    impl ReadOnly {
        fn new(dir: &Path) -> ReadOnly {
            fs::create_dir_all(dir).expect("mkdir");
            fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).expect("chmod");
            ReadOnly(dir.to_path_buf())
        }
    }

    impl Drop for ReadOnly {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn install_link_is_usr_local_bin_polygloss() {
        assert_eq!(INSTALL_LINK, "/usr/local/bin/polygloss");
    }

    #[test]
    fn install_cli_symlinks_into_writable_dir() {
        let fx = fixture();
        // `/usr/local/bin` may not exist yet: it is created.
        let link = fx.root.join("usr/local/bin/polygloss");
        assert_eq!(
            install_cli_at(&link, &fx.target, &NoAdmin).expect("install"),
            CliInstall::Linked
        );
        assert_eq!(fs::read_link(&link).expect("a symlink"), fx.target);
        // Again: nothing to do.
        assert_eq!(
            install_cli_at(&link, &fx.target, &NoAdmin).expect("again"),
            CliInstall::Unchanged
        );
        // No temporary entries are left behind.
        let names: Vec<_> = fs::read_dir(link.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [OsString::from("polygloss")]);
    }

    #[test]
    fn install_cli_replaces_stale_symlink() {
        let fx = fixture();
        let bin = fx.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let link = bin.join("polygloss");

        // A link to an app that was moved away (dangling) is replaced.
        symlink(fx.root.join("Old.app/Contents/MacOS/polygloss-cli"), &link).unwrap();
        assert_eq!(
            install_cli_at(&link, &fx.target, &NoAdmin).expect("replace dangling"),
            CliInstall::Linked
        );
        assert_eq!(fs::read_link(&link).unwrap(), fx.target);

        // A link to another existing CLI (another copy of the app) too.
        let other = fx.root.join("Other.app/Contents/MacOS/polygloss-cli");
        touch_exe(&other);
        fs::remove_file(&link).unwrap();
        symlink(&other, &link).unwrap();
        assert_eq!(
            install_cli_at(&link, &fx.target, &NoAdmin).expect("replace other"),
            CliInstall::Linked
        );
        assert_eq!(fs::read_link(&link).unwrap(), fx.target);

        // A link to a directory is replaced, not followed.
        let dir = fx.root.join("some-dir");
        fs::create_dir_all(&dir).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&dir, &link).unwrap();
        install_cli_at(&link, &fx.target, &NoAdmin).expect("replace dir link");
        assert_eq!(fs::read_link(&link).unwrap(), fx.target);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing went into it"
        );
    }

    #[test]
    fn install_cli_never_replaces_a_file_or_directory() {
        let fx = fixture();
        let bin = fx.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let link = bin.join("polygloss");

        // Something else installed a real `polygloss` there: left alone.
        fs::write(&link, "someone else's tool").unwrap();
        let err = install_cli_at(&link, &fx.target, &NoAdmin).unwrap_err();
        assert!(
            matches!(err, InstallError::Occupied(ref p) if p == &link),
            "{err:?}"
        );
        assert!(err.to_string().contains(&*link.to_string_lossy()), "{err}");
        assert_eq!(fs::read_to_string(&link).unwrap(), "someone else's tool");

        fs::remove_file(&link).unwrap();
        fs::create_dir_all(link.join("keep")).unwrap();
        let err = install_cli_at(&link, &fx.target, &NoAdmin).unwrap_err();
        assert!(matches!(err, InstallError::Occupied(_)), "{err:?}");
        assert!(link.join("keep").is_dir());
    }

    #[test]
    fn install_cli_refuses_a_missing_or_relative_target() {
        let fx = fixture();
        let link = fx.root.join("bin/polygloss");
        let missing = fx.root.join("Gone.app/Contents/MacOS/polygloss-cli");
        let err = install_cli_at(&link, &missing, &NoAdmin).unwrap_err();
        assert!(
            matches!(err, InstallError::TargetMissing(ref p) if p == &missing),
            "{err:?}"
        );
        let err = install_cli_at(&link, Path::new("polygloss-cli"), &NoAdmin).unwrap_err();
        assert!(matches!(err, InstallError::TargetMissing(_)), "{err:?}");
        assert!(!link.exists() && fs::symlink_metadata(&link).is_err());
        let err = install_cli_at(Path::new("bin/polygloss"), &fx.target, &NoAdmin).unwrap_err();
        assert!(matches!(err, InstallError::Io { .. }), "{err:?}");
    }

    #[test]
    fn install_cli_permission_denied_uses_admin_fallback() {
        let fx = fixture();
        let bin = fx.root.join("usr local/bin");
        let _ro = ReadOnly::new(&bin);
        let link = bin.join("polygloss");
        let admin = FakeAdmin::new(Outcome::Link);
        assert_eq!(
            install_cli_at(&link, &fx.target, &admin).expect("admin install"),
            CliInstall::LinkedAsAdmin
        );
        assert_eq!(fs::read_link(&link).unwrap(), fx.target);

        let calls = admin.calls();
        assert_eq!(calls.len(), 1);
        let argv = &calls[0];
        assert_eq!(argv, &admin_link_argv(&link, &fx.target));
        assert_eq!(argv[0], "/usr/bin/osascript");
        // The paths are separate argv items, never spliced into the script.
        let n = argv.len();
        assert_eq!(
            argv[n - 3..],
            [
                OsString::from(&bin),
                OsString::from(&fx.target),
                OsString::from(&link)
            ]
        );
        let script: Vec<&str> = argv[1..n - 3].iter().map(|a| a.to_str().unwrap()).collect();
        let text = script.join("\n");
        assert!(text.contains("with administrator privileges"), "{text}");
        assert!(text.contains("quoted form of"), "{text}");
        assert!(!text.contains(&*fx.root.to_string_lossy()), "{text}");

        // Also when the dir itself is missing under a read-only parent.
        let fx2 = fixture();
        let parent = fx2.root.join("usr");
        let _ro2 = ReadOnly::new(&parent);
        let link2 = parent.join("local/bin/polygloss");
        // An admin run that reports success but links nothing is an error.
        let lie = FakeAdmin::new(Outcome::LieSuccess);
        let err = install_cli_at(&link2, &fx2.target, &lie).unwrap_err();
        assert!(
            matches!(err, InstallError::NotLinked(ref p) if p == &link2),
            "{err:?}"
        );
        assert_eq!(lie.calls(), [admin_link_argv(&link2, &fx2.target)]);
    }

    #[test]
    fn install_cli_admin_cancel_is_reported_as_cancelled() {
        let fx = fixture();
        let bin = fx.root.join("bin");
        let _ro = ReadOnly::new(&bin);
        let link = bin.join("polygloss");
        let admin = FakeAdmin::new(Outcome::Cancel);
        let err = install_cli_at(&link, &fx.target, &admin).unwrap_err();
        assert!(matches!(err, InstallError::Cancelled), "{err:?}");
        assert!(fs::symlink_metadata(&link).is_err());
    }

    /// The real script, run by the real `osascript` without the admin
    /// clause, links hostile paths exactly: the quoting holds.
    #[test]
    fn admin_script_quotes_hostile_paths() {
        let fx = fixture();
        let weird = fx.root.join("it's a \"dir\" $(touch pwned) `x` \\ ;&|");
        let target = weird.join("Polygloss.app/Contents/MacOS/polygloss-cli");
        touch_exe(&target);
        let link = weird.join("bin dir/polygloss");
        let argv: Vec<OsString> = admin_link_argv(&link, &target)
            .into_iter()
            .map(|a| {
                let s = a.to_string_lossy();
                if s.contains("with administrator privileges") {
                    s.replace(" with administrator privileges", "").into()
                } else {
                    a
                }
            })
            .collect();
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&fx.root)
            .env("HOME", &fx.root)
            .output()
            .expect("run osascript");
        assert!(
            out.status.success(),
            "osascript: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert!(!fx.root.join("pwned").exists());
        assert!(!weird.join("pwned").exists());
    }

    /// The exact admin script (with its administrator clause) compiles;
    /// `osacompile` only compiles, so no prompt appears.
    #[test]
    fn admin_script_compiles() {
        let fx = fixture();
        let argv = admin_link_argv(&fx.root.join("bin/polygloss"), &fx.target);
        let script_args = &argv[1..argv.len() - 3];
        let out = std::process::Command::new("/usr/bin/osacompile")
            .arg("-o")
            .arg(fx.root.join("install.scpt"))
            .args(script_args)
            .env("HOME", &fx.root)
            .output()
            .expect("run osacompile");
        assert!(
            out.status.success(),
            "osacompile: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
