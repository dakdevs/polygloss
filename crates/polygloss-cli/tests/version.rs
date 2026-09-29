use std::process::Command;

#[test]
fn cli_prints_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_polygloss-cli"))
        .arg("--version")
        .output()
        .expect("run polygloss-cli");
    assert!(out.status.success(), "exit status {:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("polygloss {}\n", env!("CARGO_PKG_VERSION"))
    );
}
