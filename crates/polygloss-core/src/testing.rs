//! Test support (feature `test-support`): `Sandbox` and `FixtureRepo` (T1.2).
//!
//! `Sandbox::isolate()` points `HOME`, `POLYGLOSS_DATA_DIR`, `XDG_CONFIG_HOME`,
//! `XDG_CACHE_HOME` and git's global config at a temp dir, so tests never read or
//! write the real `~/Library` or `~/.config`. It sets **process** env, which is only
//! sound because nextest runs every test in its own process (plan M1 conventions).
//! It leaves a caller-set `POLYGLOSS_GIT_BIN` alone, so
//! `POLYGLOSS_GIT_BIN=<older git> scripts/cargo.sh nextest run -p polygloss-core`
//! runs the runner under test against that git (plan risk S13). Fixtures always use
//! `git` from `PATH`, so tests that point the runner at a fake git can still build
//! repos.
//!
//! `FixtureRepo` builds small git repos with a hermetic git: no system config, the
//! sandbox's (or an empty) global config, fixed identity and per-commit dates.

// The only module in `polygloss-core` allowed to use `unsafe`: Rust 2024 makes
// `std::env::set_var` unsafe, and `isolate()` must set process env.
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use polygloss_diff::{ObjectFormat, Oid};
use tempfile::TempDir;

/// Inherited repo-redirecting env that `isolate()` and fixtures clear (the runner
/// clears these too, plus inherited per-invocation config).
const SCRUBBED_GIT_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// A per-test temp `HOME`, data dir, config dir, cache dir and empty git config.
/// Keep it alive for the whole test (`let _sb = Sandbox::isolate();`); dropping it
/// deletes the temp dir. Env is not restored (one process per test).
pub struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    data_dir: PathBuf,
    config_dir: PathBuf,
    cache_dir: PathBuf,
    git_config_global: PathBuf,
}

impl Sandbox {
    pub fn isolate() -> Sandbox {
        let root = tempfile::Builder::new()
            .prefix("polygloss-sandbox-")
            .tempdir()
            .expect("create sandbox temp dir");
        let base = std::fs::canonicalize(root.path()).expect("canonicalize sandbox");
        let home = base.join("home");
        let data_dir = base.join("data");
        let config_dir = home.join(".config");
        let cache_dir = home.join(".cache");
        for dir in [&home, &data_dir, &config_dir, &cache_dir] {
            std::fs::create_dir_all(dir).expect("create sandbox dir");
        }
        let git_config_global = home.join(".gitconfig-empty");
        std::fs::write(&git_config_global, "").expect("create empty git config");

        // SAFETY: nextest runs each test in its own process, and `isolate()` is the
        // first thing a test does, before any other thread reads the environment.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("POLYGLOSS_DATA_DIR", &data_dir);
            std::env::set_var("XDG_CONFIG_HOME", &config_dir);
            std::env::set_var("XDG_CACHE_HOME", &cache_dir);
            std::env::set_var("GIT_CONFIG_GLOBAL", &git_config_global);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            for var in SCRUBBED_GIT_ENV {
                std::env::remove_var(var);
            }
        }

        Sandbox {
            _root: root,
            home,
            data_dir,
            config_dir,
            cache_dir,
            git_config_global,
        }
    }

    /// The temp `HOME` (canonical path).
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `POLYGLOSS_DATA_DIR`.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// `XDG_CONFIG_HOME` (`<home>/.config`).
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// `XDG_CACHE_HOME` (`<home>/.cache`).
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// The (initially empty) file `GIT_CONFIG_GLOBAL` points at. Tests may write
    /// global settings such as `core.excludesFile` into it.
    pub fn git_config_global(&self) -> &Path {
        &self.git_config_global
    }
}

/// Fixed commit dates: 2026-01-01T00:00:00Z plus one minute per commit.
const EPOCH: u64 = 1_767_225_600;

/// A throwaway git repo in its own temp dir (`<tmp>/repo`; linked worktrees go in
/// `<tmp>/worktrees/<name>`). All paths are canonical.
pub struct FixtureRepo {
    root: TempDir,
    path: PathBuf,
    commits: AtomicU32,
}

impl FixtureRepo {
    /// `git init -b main --object-format=<fmt>`.
    pub fn init(fmt: ObjectFormat) -> FixtureRepo {
        let root = new_root();
        let base = std::fs::canonicalize(root.path()).expect("canonicalize fixture root");
        let path = base.join("repo");
        run_git(
            &base,
            0,
            [
                OsStr::new("init"),
                OsStr::new("-q"),
                OsStr::new("-b"),
                OsStr::new("main"),
                OsStr::new(&format!("--object-format={}", fmt.as_str())),
                path.as_os_str(),
            ],
        );
        FixtureRepo {
            root,
            path,
            commits: AtomicU32::new(0),
        }
    }

    /// The repo's worktree root (canonical).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `bytes` to `rel` inside the worktree, creating parent dirs.
    pub fn write(&self, rel: &str, bytes: &[u8]) {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create fixture dirs");
        }
        std::fs::write(&full, bytes).expect("write fixture file");
    }

    /// `git add -A && git commit --allow-empty -m <msg>`; returns the new HEAD.
    pub fn commit(&self, msg: &str) -> Oid {
        self.git(&["add", "-A"]);
        let n = self.commits.fetch_add(1, Ordering::SeqCst) + 1;
        run_git(
            &self.path,
            n,
            ["commit", "-q", "--allow-empty", "--no-verify", "-m", msg],
        );
        self.oid("HEAD")
    }

    /// Runs git in the worktree, panicking with stderr on failure; returns stdout
    /// with trailing newlines removed.
    pub fn git(&self, args: &[&str]) -> String {
        let n = self.commits.load(Ordering::SeqCst);
        let out = run_git(&self.path, n, args);
        String::from_utf8_lossy(&out)
            .trim_end_matches('\n')
            .to_owned()
    }

    /// `git branch <name>` at HEAD.
    pub fn branch(&self, name: &str) {
        self.git(&["branch", name]);
    }

    /// `git checkout <name>`.
    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    /// Adds a linked worktree at `<tmp>/worktrees/<name>` on branch `name` (created
    /// from HEAD when missing); returns its canonical path.
    pub fn add_worktree(&self, name: &str) -> PathBuf {
        let dir = self.root_path().join("worktrees");
        std::fs::create_dir_all(&dir).expect("create worktrees dir");
        let wt = dir.join(name);
        let wt_str = wt.to_str().expect("utf-8 temp path");
        let exists = run_git_status(
            &self.path,
            ["show-ref", "--verify", "-q", &format!("refs/heads/{name}")],
        );
        if exists {
            self.git(&["worktree", "add", "-q", wt_str, name]);
        } else {
            self.git(&["worktree", "add", "-q", "-b", name, wt_str]);
        }
        std::fs::canonicalize(&wt).expect("canonicalize worktree")
    }

    /// `git clone --depth 1 file://<repo>` into a new temp dir.
    pub fn clone_shallow(&self) -> FixtureRepo {
        self.clone_with(&["--depth", "1"])
    }

    /// `git clone --filter=blob:none file://<repo>` into a new temp dir (a partial
    /// clone whose older blobs are missing; `uploadpack.allowFilter` is enabled on
    /// the source).
    pub fn clone_blobless(&self) -> FixtureRepo {
        self.git(&["config", "uploadpack.allowFilter", "true"]);
        self.clone_with(&["--filter=blob:none"])
    }

    /// Resolves `rev` in this repo to an `Oid`.
    pub fn oid(&self, rev: &str) -> Oid {
        let fmt = ObjectFormat::from_name(&self.git(&["rev-parse", "--show-object-format"]))
            .expect("known object format");
        let text = self.git(&["rev-parse", "--verify", "--end-of-options", rev]);
        Oid::parse(&text, fmt).expect("rev-parse prints an oid")
    }

    fn root_path(&self) -> PathBuf {
        std::fs::canonicalize(self.root.path()).expect("canonicalize fixture root")
    }

    fn clone_with(&self, flags: &[&str]) -> FixtureRepo {
        let root = new_root();
        let base = std::fs::canonicalize(root.path()).expect("canonicalize fixture root");
        let path = base.join("repo");
        let url = format!("file://{}", self.path.display());
        let mut args: Vec<&OsStr> = vec![OsStr::new("clone"), OsStr::new("-q")];
        args.extend(flags.iter().map(OsStr::new));
        args.push(OsStr::new(&url));
        args.push(path.as_os_str());
        run_git(&base, 0, args);
        FixtureRepo {
            root,
            path,
            commits: AtomicU32::new(self.commits.load(Ordering::SeqCst)),
        }
    }
}

fn new_root() -> TempDir {
    tempfile::Builder::new()
        .prefix("polygloss-fixture-")
        .tempdir()
        .expect("create fixture temp dir")
}

/// A hermetic git command in `dir`: no system config, the sandbox's global config
/// (or `/dev/null` outside a sandbox), fixed identity and a date for commit `n`.
fn fixture_git(dir: &Path, n: u32) -> Command {
    let date = format!("@{} +0000", EPOCH + u64::from(n) * 60);
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .stdin(Stdio::null())
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Polygloss Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@polygloss.invalid")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", "Polygloss Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@polygloss.invalid")
        .env("GIT_COMMITTER_DATE", &date);
    if std::env::var_os("GIT_CONFIG_GLOBAL").is_none() {
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    }
    for var in SCRUBBED_GIT_ENV {
        cmd.env_remove(var);
    }
    cmd
}

fn run_git<I, S>(dir: &Path, n: u32, args: I) -> Vec<u8>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let out = fixture_git(dir, n)
        .args(&args)
        .output()
        .expect("spawn fixture git");
    assert!(
        out.status.success(),
        "fixture git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn run_git_status<I, S>(dir: &Path, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    fixture_git(dir, 0)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn fixture git")
        .success()
}

/// How many git processes with this subcommand (`diff-tree`, `rev-parse`, …) the
/// runner (`git::Git`) has spawned in this process. A test hook for "served from
/// the cache, no git ran" assertions (T1.12).
pub fn git_spawns(subcommand: &str) -> usize {
    crate::git::runner::spawn_count(subcommand)
}

/// Makes every `Core` pin in this process sleep `pause` after `Snapshotter::pin`
/// created the snapshot ref and before the iteration naming it commits (while the
/// per-repo pin/prune guard is held), so tests can race a prune against a pin
/// (T1.12). `Duration::ZERO` turns it off.
pub fn pause_after_snapshot_pin(pause: std::time::Duration) {
    crate::review::open::set_pin_pause(pause);
}
