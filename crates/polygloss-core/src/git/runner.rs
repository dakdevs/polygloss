//! `Git`: the system git runner with the offline env, scrubbed inherited env and pinned `-c` flags (T1.2).
//!
//! Every git process Polygloss starts goes through here (design §6.2, plan Global
//! constraints "Offline"): `git -C <worktree> -c protocol.allow=never
//! -c core.quotePath=false <args>` with `GIT_NO_LAZY_FETCH=1`, `GIT_TERMINAL_PROMPT=0`,
//! `GIT_OPTIONAL_LOCKS=0` and `LC_ALL=C`, and with the inherited repo-redirecting
//! variables cleared. `with_env` may set those variables again on purpose (snapshots).
//! The user's global and system config stay enabled (design §6.2).

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::git::version::GitVersion;

/// Inherited variables that would point git at another repo, index or object store.
const SCRUBBED_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Offline and deterministic-output env set on every call.
const PINNED_ENV: [(&str, &str); 4] = [
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("LC_ALL", "C"),
];

/// `-c` flags passed before every subcommand.
const PINNED_CONFIG: [&str; 2] = ["protocol.allow=never", "core.quotePath=false"];

/// Errors from running git or interpreting its output.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git ({}): {source}", bin.display())]
    Spawn {
        bin: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("git {args} failed (exit {}): {}", code.map_or_else(|| "signal".to_owned(), |c| c.to_string()), stderr.trim_end())]
    Failed {
        args: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("git {found} is too old; Polygloss needs git {min} or newer")]
    TooOld { found: GitVersion, min: GitVersion },
    #[error("{} is not inside a git repository", .0.display())]
    NotARepo(PathBuf),
    #[error(
        "no default branch: none of refs/remotes/origin/HEAD, origin/main, origin/master, main or master exists"
    )]
    NoDefaultBranch,
    #[error("unexpected git output: {0}")]
    Parse(String),
    #[error("git I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// The raw result of one git process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    /// Exit code; `None` when git was killed by a signal.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// The git binary: `$POLYGLOSS_GIT_BIN` (tests and CI only) or `git` from `PATH`.
pub fn git_binary() -> PathBuf {
    std::env::var_os("POLYGLOSS_GIT_BIN")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("git"), PathBuf::from)
}

/// A git invocation context: a worktree (or git dir) for `-C` plus extra env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    worktree: PathBuf,
    env: Vec<(String, OsString)>,
}

impl Git {
    pub fn new(worktree: impl Into<PathBuf>) -> Git {
        Git {
            worktree: worktree.into(),
            env: Vec::new(),
        }
    }

    /// Sets `key` for every call, after the scrub (so it may re-set `GIT_INDEX_FILE`
    /// and friends on purpose). A later value for the same key replaces an earlier one.
    pub fn with_env(mut self, key: &str, val: impl AsRef<OsStr>) -> Git {
        self.env.retain(|(k, _)| k != key);
        self.env.push((key.to_owned(), val.as_ref().to_owned()));
        self
    }

    /// The directory passed to `git -C`.
    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    /// Runs git and returns stdout; a non-zero exit is `GitError::Failed`.
    pub fn output(&self, args: &[&OsStr]) -> Result<Vec<u8>, GitError> {
        let out = self.run_with(args, None)?;
        check(args, out)
    }

    /// Like `output`, feeding `stdin` to the process.
    pub fn output_stdin(&self, args: &[&OsStr], stdin: &[u8]) -> Result<Vec<u8>, GitError> {
        let out = self.run_with(args, Some(stdin))?;
        check(args, out)
    }

    /// Runs git and returns its exit code without mapping it to an error
    /// (`symbolic-ref -q`, `merge-base` and friends use exit 1 as an answer).
    /// Killed by a signal is `GitError::Failed` with `code: None`.
    pub fn status(&self, args: &[&OsStr]) -> Result<i32, GitError> {
        let out = self.run(args)?;
        out.code.ok_or_else(|| failed(args, &out))
    }

    /// Runs git and returns exit code, stdout and stderr unmapped.
    pub fn run(&self, args: &[&OsStr]) -> Result<GitOutput, GitError> {
        self.run_with(args, None)
    }

    fn run_with(&self, args: &[&OsStr], stdin: Option<&[u8]>) -> Result<GitOutput, GitError> {
        let mut cmd = base_command(Some(&self.worktree));
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        record_spawn(args);
        let bin = git_binary();
        let mut child = cmd.spawn().map_err(|source| GitError::Spawn {
            bin: bin.clone(),
            source,
        })?;
        // Feed stdin from a thread so a large input can never deadlock against a
        // full stdout pipe.
        let writer = match (stdin, child.stdin.take()) {
            (Some(bytes), Some(mut pipe)) => {
                let bytes = bytes.to_vec();
                Some(std::thread::spawn(move || pipe.write_all(&bytes)))
            }
            _ => None,
        };
        let out = child.wait_with_output()?;
        if let Some(writer) = writer {
            match writer.join() {
                Ok(Ok(())) => {}
                // git may exit before reading all input (e.g. on an early error);
                // its exit status reports that.
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Ok(Err(e)) => return Err(GitError::Io(e)),
                Err(_) => return Err(GitError::Parse("stdin writer thread panicked".into())),
            }
        }
        Ok(GitOutput {
            code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

/// `git [-C dir] -c … ` with the pinned env and the scrub applied; used by `Git` and
/// by `check_version` (which has no worktree).
pub(crate) fn base_command(dir: Option<&Path>) -> Command {
    let mut cmd = Command::new(git_binary());
    for var in SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    for (k, v) in PINNED_ENV {
        cmd.env(k, v);
    }
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    for c in PINNED_CONFIG {
        cmd.arg("-c").arg(c);
    }
    cmd
}

fn check(args: &[&OsStr], out: GitOutput) -> Result<Vec<u8>, GitError> {
    if out.success() {
        Ok(out.stdout)
    } else {
        Err(failed(args, &out))
    }
}

fn failed(args: &[&OsStr], out: &GitOutput) -> GitError {
    GitError::Failed {
        args: args
            .iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
        code: out.code,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[cfg(feature = "test-support")]
static SPAWNS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(feature = "test-support")]
fn record_spawn(args: &[&OsStr]) {
    let sub = args
        .first()
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or_default();
    SPAWNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(sub);
}

#[cfg(not(feature = "test-support"))]
fn record_spawn(_args: &[&OsStr]) {}

/// Test hook behind `testing::git_spawns`.
#[cfg(feature = "test-support")]
pub(crate) fn spawn_count(subcommand: &str) -> usize {
    SPAWNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|s| *s == subcommand)
        .count()
}
