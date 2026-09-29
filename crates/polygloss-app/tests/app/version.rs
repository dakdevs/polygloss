use std::path::Path;
use std::process::Command;

/// `bin` with a throwaway HOME, config, cache, data dir and git config, so the
/// test never touches the real `~/Library`, `~/.config` or git config.
fn sandboxed(bin: &str, root: &Path) -> Command {
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".cache")).expect("create sandbox HOME");
    let git_config = root.join("gitconfig");
    std::fs::write(&git_config, "").expect("write empty gitconfig");
    let mut cmd = Command::new(bin);
    cmd.env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("POLYGLOSS_DATA_DIR", root.join("data"))
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

#[test]
fn app_prints_version_and_exits_zero() {
    let root = tempfile::tempdir().expect("temp dir");
    let out = sandboxed(env!("CARGO_BIN_EXE_Polygloss"), root.path())
        .arg("--version")
        .output()
        .expect("run Polygloss");
    assert!(out.status.success(), "exit status {:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("Polygloss {}\n", env!("CARGO_PKG_VERSION"))
    );
}
