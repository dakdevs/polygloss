//! Git version parsing and the minimum-version check (T1.2, OQ-6).

use std::fmt;
use std::process::Stdio;

use crate::git::runner::{GitError, base_command, spawn_error};

/// A system git version (`git --version`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GitVersion {
    /// The oldest supported git: 2.39.0, the Xcode Command Line Tools git (provisional, OQ-6).
    pub const MIN: GitVersion = GitVersion {
        major: 2,
        minor: 39,
        patch: 0,
    };

    /// Parses `git version 2.54.0 (Apple Git-157)`, `git version 2.45.1.windows.1`,
    /// `git version 2.50.0.rc1` and the like. A missing patch component is 0.
    pub fn parse(s: &str) -> Result<GitVersion, GitError> {
        let bad = || GitError::Parse(format!("not a git version line: {:?}", s.trim_end()));
        let rest = s.trim().strip_prefix("git version ").ok_or_else(bad)?;
        let token = rest.split_whitespace().next().ok_or_else(bad)?;
        let mut parts = token.split('.');
        let mut num = |required: bool| -> Result<u32, GitError> {
            match parts.next() {
                Some(p) => {
                    let digits: &str =
                        &p[..p.find(|c: char| !c.is_ascii_digit()).unwrap_or(p.len())];
                    if digits.is_empty() {
                        if required { Err(bad()) } else { Ok(0) }
                    } else {
                        digits.parse().map_err(|_| bad())
                    }
                }
                None if required => Err(bad()),
                None => Ok(0),
            }
        };
        Ok(GitVersion {
            major: num(true)?,
            minor: num(true)?,
            patch: num(false)?,
        })
    }

    /// True when this version is `major.minor` or newer.
    pub fn at_least(&self, major: u32, minor: u32) -> bool {
        (self.major, self.minor) >= (major, minor)
    }
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Runs `git --version` and checks it against `GitVersion::MIN`; an older git is
/// `GitError::TooOld`. Call at startup and show the error to the user.
pub fn check_version() -> Result<GitVersion, GitError> {
    let mut cmd = base_command(None);
    cmd.arg("--version").stdin(Stdio::null());
    let out = cmd.output().map_err(|source| spawn_error(&cmd, source))?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: "--version".into(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    let found = GitVersion::parse(&String::from_utf8_lossy(&out.stdout))?;
    if found < GitVersion::MIN {
        return Err(GitError::TooOld {
            found,
            min: GitVersion::MIN,
        });
    }
    Ok(found)
}
