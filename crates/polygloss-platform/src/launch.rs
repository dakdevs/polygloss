//! Launching the app for the CLI and the MCP server (T4.2, design §13.4).
//!
//! T4.4 placeholder with T4.2's names (`Launcher`, `SystemLauncher`). T4.2's
//! module replaces this file on merge.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The app's bundle id.
pub const BUNDLE_ID: &str = "dev.dak.polygloss";

/// Starts the app.
pub trait Launcher {
    fn launch(&self, url: Option<&str>, activate: bool) -> io::Result<()>;
}

/// Starts the real app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemLauncher {
    /// `open [-g] -b dev.dak.polygloss [url]`.
    Open,
    /// `$POLYGLOSS_APP_BIN` under `POLYGLOSS_TEST=1`.
    AppBin(PathBuf),
}

impl SystemLauncher {
    pub fn from_env() -> SystemLauncher {
        let test = std::env::var_os("POLYGLOSS_TEST").is_some_and(|v| v == "1");
        match std::env::var_os("POLYGLOSS_APP_BIN").filter(|v| !v.is_empty()) {
            Some(bin) if test => SystemLauncher::AppBin(bin.into()),
            _ => SystemLauncher::Open,
        }
    }
}

impl Launcher for SystemLauncher {
    fn launch(&self, url: Option<&str>, activate: bool) -> io::Result<()> {
        let mut argv: Vec<OsString> = match self {
            SystemLauncher::Open => {
                let mut a = vec![OsString::from("/usr/bin/open")];
                if !activate {
                    a.push("-g".into());
                }
                a.extend(["-b".into(), BUNDLE_ID.into()]);
                a
            }
            SystemLauncher::AppBin(bin) => vec![bin.clone().into_os_string()],
        };
        argv.extend(url.map(OsString::from));
        Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    }
}
