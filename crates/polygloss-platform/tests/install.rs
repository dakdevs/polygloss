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
