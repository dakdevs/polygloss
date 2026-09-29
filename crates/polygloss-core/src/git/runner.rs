//! `Git`: the system git runner with the offline env, scrubbed inherited env and pinned `-c` flags (T1.2).
//!
//! Every git process Polygloss starts goes through here (design §6.2, plan Global
//! constraints "Offline"): `git -C <worktree> -c protocol.allow=never
//! -c core.quotePath=false <args>` with `GIT_NO_LAZY_FETCH=1`, `GIT_TERMINAL_PROMPT=0`,
//! `GIT_OPTIONAL_LOCKS=0`, `GIT_ALLOW_PROTOCOL=` (empty), `GIT_NO_REPLACE_OBJECTS=1`
//! and `LC_ALL=C`, and with the inherited repo-redirecting variables and inherited
//! per-invocation config (`GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`) cleared.
//! `with_env` may set those variables again on purpose (snapshots). The user's
//! global and system config stay enabled (design §6.2).
//!
//! Why the extra pins: `protocol.<name>.allow` (user config, `git -c` parents, or
//! `GIT_CONFIG_COUNT`/`GIT_CONFIG_PARAMETERS` from agents and hooks) overrides
//! `protocol.allow=never`, and an inherited `GIT_ALLOW_PROTOCOL` overrides both. An
//! empty `GIT_ALLOW_PROTOCOL` allows no transport whatever the config says, which is
//! what keeps git 2.39–2.43 (no `GIT_NO_LAZY_FETCH`) from lazily fetching in a
//! partial clone. `GIT_NO_REPLACE_OBJECTS=1` makes revisions, trees and therefore
//! `diff_id` come from the real objects, never from `refs/replace/*`, which clones do
//! not share.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::git::version::GitVersion;

/// Inherited variables that would point git at another repo, index or object
/// store, or inject per-invocation config (`GIT_CONFIG_KEY_<n>`/`VALUE_<n>` are
/// ignored without `GIT_CONFIG_COUNT`).
const SCRUBBED_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

/// Offline and deterministic-output env set on every call.
const PINNED_ENV: [(&str, &str); 6] = [
    ("GIT_NO_LAZY_FETCH", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    // Empty list: no transport is allowed, overriding every `protocol.*` setting.
    ("GIT_ALLOW_PROTOCOL", ""),
    ("GIT_NO_REPLACE_OBJECTS", "1"),
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
///
/// On macOS a `PATH` whose `git` is the Xcode command-line-tools shim
/// (`/usr/bin/git`) gets the git that shim runs instead: `usr/bin/git` in the
/// selected developer directory (`xcode-select -p`, which honors
/// `DEVELOPER_DIR` like the shim does), asked once per process. The shim looks
/// its tool up on every call, which costs about 9 ms per git process, several
/// times what git itself takes for the plumbing calls of an open (plan
/// T2.10.1). It is the same system git (design §6.2), minus the trampoline.
/// (`xcrun --find git` gives the same answer but takes ≈ 75 ms in a fresh
/// `HOME`, where its cache is cold, as in every perf run.)
pub fn git_binary() -> PathBuf {
    if let Some(bin) = std::env::var_os("POLYGLOSS_GIT_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(bin);
    }
    let path = std::env::var_os("PATH");
    match XCODE_GIT_SHIM {
        Some(shim) => default_git(path.as_deref(), Path::new(shim), xcode_git),
        None => PathBuf::from("git"),
    }
}

/// The Xcode command-line-tools shim, which runs the selected developer
/// directory's git.
const XCODE_GIT_SHIM: Option<&str> = if cfg!(target_os = "macos") {
    Some("/usr/bin/git")
} else {
    None
};

/// The git the Xcode shim runs: `<developer dir>/usr/bin/git`, the developer
/// dir from `xcode-select -p`; asked once per process.
fn xcode_git() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let out = Command::new("/usr/bin/xcode-select")
                .arg("-p")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())?;
            let dir = std::str::from_utf8(&out.stdout).ok()?.trim_end();
            Some(Path::new(dir).join("usr/bin/git"))
        })
        .clone()
}

/// What a bare `git` should be spawned as: the git `find_real` names when the
/// first executable `git` on `path` is `shim` and the answer is another
/// executable file at an absolute path; otherwise `git`, looked up on `PATH`
/// by the spawn as before.
fn default_git(
    path: Option<&OsStr>,
    shim: &Path,
    find_real: impl FnOnce() -> Option<PathBuf>,
) -> PathBuf {
    let plain = || PathBuf::from("git");
    let Some(first) = path.and_then(|p| {
        std::env::split_paths(p)
            .map(|d| d.join("git"))
            .find(|g| is_executable(g))
    }) else {
        return plain();
    };
    if first != shim {
        return plain();
    }
    match find_real() {
        Some(real) if real.is_absolute() && real != shim && is_executable(&real) => real,
        _ => plain(),
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
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

    /// Like `run`, feeding `stdin` to the process (`check-ignore --stdin`
    /// exits 1 as an answer).
    pub fn run_stdin(&self, args: &[&OsStr], stdin: &[u8]) -> Result<GitOutput, GitError> {
        self.run_with(args, Some(stdin))
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
        let mut child = cmd.spawn().map_err(|source| spawn_error(&cmd, source))?;
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

/// `cmd` could not start: names the binary it ran (what [`git_binary`] chose
/// for it, without looking it up again).
pub(crate) fn spawn_error(cmd: &Command, source: std::io::Error) -> GitError {
    GitError::Spawn {
        bin: PathBuf::from(cmd.get_program()),
        source,
    }
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// An executable file at `dir/name`.
    fn exe(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn default_git_replaces_the_xcode_shim_with_the_git_it_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let shim = exe(&tmp.path().join("usr-bin"), "git");
        let real = exe(&tmp.path().join("developer/usr/bin"), "git");
        let path = path_of(&[&empty, shim.parent().unwrap()]);
        let asked = Cell::new(0);
        let found = default_git(Some(&path), &shim, || {
            asked.set(asked.get() + 1);
            Some(real.clone())
        });
        assert_eq!(found, real);
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn default_git_keeps_the_path_lookup_for_any_other_git() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = exe(&tmp.path().join("usr-bin"), "git");
        let brew = exe(&tmp.path().join("brew/bin"), "git");
        // Not executable: skipped by the lookup, like the OS skips it.
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("git"), "").unwrap();
        let path = path_of(&[&plain, brew.parent().unwrap(), shim.parent().unwrap()]);
        let never = || -> Option<PathBuf> { panic!("looked up the shim's git for another git") };
        assert_eq!(default_git(Some(&path), &shim, never), PathBuf::from("git"));
        // No git on PATH, or no PATH: `git` as before (the spawn reports it).
        let none = path_of(&[&plain]);
        assert_eq!(default_git(Some(&none), &shim, never), PathBuf::from("git"));
        assert_eq!(default_git(None, &shim, never), PathBuf::from("git"));
    }

    #[test]
    fn default_git_keeps_the_shim_when_the_lookup_has_no_usable_answer() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = exe(&tmp.path().join("usr-bin"), "git");
        let path = path_of(&[shim.parent().unwrap()]);
        let missing = tmp.path().join("gone/git");
        let not_exe = tmp.path().join("not-exe");
        std::fs::write(&not_exe, "").unwrap();
        for answer in [
            None,
            Some(PathBuf::from("git")),
            Some(missing),
            Some(not_exe),
            Some(shim.clone()),
        ] {
            let found = default_git(Some(&path), &shim, || answer.clone());
            assert_eq!(
                found,
                PathBuf::from("git"),
                "the lookup answered {answer:?}"
            );
        }
    }
}
