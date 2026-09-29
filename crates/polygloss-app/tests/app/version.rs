use std::process::Command;

#[test]
fn app_prints_version_and_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_Polygloss"))
        .arg("--version")
        .output()
        .expect("run Polygloss");
    assert!(out.status.success(), "exit status {:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("Polygloss {}\n", env!("CARGO_PKG_VERSION"))
    );
}
