//! Git runner, version check, repo discovery and source resolution (T1.2, design §3,
//! §4.2, §4.3, §6.2). Every test runs under `Sandbox::isolate()`.
#![allow(unsafe_code)] // tests set process env (one process per test under nextest)

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use polygloss_core::git::{
    CompareMode, Git, GitError, GitVersion, HeadSpec, ResolveError, ResolveWarning, ResolvedSide,
    ReviewKind, Since, Source, check_version, default_branch, discover, git_binary, resolve,
};
use polygloss_core::testing::{FixtureRepo, Sandbox, git_spawns};
use polygloss_core::{ObjectFormat, Oid};

fn os<'a>(args: &'a [&'a str]) -> Vec<&'a OsStr> {
    args.iter().map(OsStr::new).collect()
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap().trim_end().to_owned()
}

fn set_env(key: &str, val: impl AsRef<OsStr>) {
    // SAFETY: one process per test (nextest); no other threads read env here.
    unsafe { std::env::set_var(key, val) }
}

/// A repo with commit `c1` (a.txt) on `main`.
fn one_commit(fmt: ObjectFormat) -> (FixtureRepo, Oid) {
    let repo = FixtureRepo::init(fmt);
    repo.write("a.txt", b"one\n");
    let c1 = repo.commit("c1");
    (repo, c1)
}

fn tree(repo: &FixtureRepo, rev: &str) -> Oid {
    repo.oid(&format!("{rev}^{{tree}}"))
}

fn resolve_in(
    repo: &FixtureRepo,
    source: Source,
) -> Result<polygloss_core::git::Resolution, ResolveError> {
    let info = discover(repo.path()).unwrap();
    resolve(&info, repo.path(), &source)
}

fn head_side(head: &HeadSpec) -> &ResolvedSide {
    match head {
        HeadSpec::Tree(side) => side,
        HeadSpec::Worktree => panic!("expected a tree head, got the worktree"),
    }
}

/// Writes an executable fake `git` that records its argv and env next to itself
/// and always prints `version_line` (the pinned `-c` flags precede `--version`).
fn fake_git(dir: &Path, version_line: &str) -> PathBuf {
    let script = dir.join("fake-git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$d/args.txt\"\nenv > \"$d/env.txt\"\necho '{version_line}'\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

// ---------------------------------------------------------------- runner

#[test]
fn git_runner_scrubs_inherited_git_env() {
    let sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    let (other, _) = one_commit(ObjectFormat::Sha1);
    other.write("b.txt", b"other\n");
    other.commit("other c2");

    // What agents and hooks export (RF3): every one points at the wrong repo.
    let bogus = sb.home().join("bogus");
    set_env("GIT_DIR", other.path().join(".git"));
    set_env("GIT_WORK_TREE", other.path());
    set_env("GIT_COMMON_DIR", other.path().join(".git"));
    set_env("GIT_INDEX_FILE", bogus.join("index"));
    set_env("GIT_OBJECT_DIRECTORY", bogus.join("objects"));
    set_env("GIT_ALTERNATE_OBJECT_DIRECTORIES", bogus.join("alt"));

    let git = Git::new(repo.path());
    let top = text(git.output(&os(&["rev-parse", "--show-toplevel"])).unwrap());
    assert_eq!(Path::new(&top), repo.path());
    let head = text(git.output(&os(&["rev-parse", "HEAD"])).unwrap());
    assert_eq!(head, c1.as_str());
    let index = text(
        git.output(&os(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index",
        ]))
        .unwrap(),
    );
    assert_eq!(Path::new(&index), repo.path().join(".git/index"));

    let info = discover(repo.path()).unwrap();
    assert_eq!(info.common_dir, repo.path().join(".git"));

    // `with_env` sets them on purpose (snapshots, T1.5) and wins over the scrub.
    let temp_index = sb.home().join("temp-index");
    let redirected = Git::new(repo.path()).with_env("GIT_INDEX_FILE", &temp_index);
    let index = text(
        redirected
            .output(&os(&[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "index",
            ]))
            .unwrap(),
    );
    assert_eq!(Path::new(&index), temp_index);
}

#[test]
fn git_runner_sets_offline_env_and_flags() {
    let sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);

    // Real git sees the pinned `-c` flags.
    let git = Git::new(repo.path());
    let get = |key: &str| text(git.output(&os(&["config", "--get", key])).unwrap());
    assert_eq!(get("protocol.allow"), "never");
    assert_eq!(get("core.quotePath"), "false");
    // ... so even a local file:// transport is refused: nothing can ever fetch.
    let url = format!("file://{}", repo.path().display());
    let err = git.output(&os(&["ls-remote", &url])).unwrap_err();
    assert!(matches!(err, GitError::Failed { .. }), "{err:?}");

    // A recording fake git shows the exact argv and env.
    let fake_dir = sb.home().join("fake");
    std::fs::create_dir_all(&fake_dir).unwrap();
    let fake = fake_git(&fake_dir, "git version 2.54.0");
    set_env("POLYGLOSS_GIT_BIN", &fake);
    set_env("LC_ALL", "en_US.UTF-8");
    set_env("GIT_TERMINAL_PROMPT", "1");
    set_env("GIT_DIR", "/nonexistent/.git");
    set_env("GIT_ALLOW_PROTOCOL", "file:ssh");
    set_env("GIT_NO_REPLACE_OBJECTS", "0");
    set_env("GIT_CONFIG_PARAMETERS", "'protocol.allow'='always'");
    set_env("GIT_CONFIG_COUNT", "1");
    assert_eq!(git_binary(), fake);

    Git::new(repo.path())
        .with_env("GIT_INDEX_FILE", "/tmp/polygloss-test-index")
        .output(&os(&["status", "--porcelain"]))
        .unwrap();
    let args = std::fs::read_to_string(fake_dir.join("args.txt")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let repo_path = repo.path().to_str().unwrap();
    assert_eq!(
        args,
        [
            "-C",
            repo_path,
            "-c",
            "protocol.allow=never",
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain"
        ]
    );
    let env = std::fs::read_to_string(fake_dir.join("env.txt")).unwrap();
    // Only git-related lines: the full env may hold secrets, never print it.
    let env: Vec<&str> = env
        .lines()
        .filter(|l| l.starts_with("GIT_") || l.starts_with("LC_"))
        .collect();
    for want in [
        "GIT_NO_LAZY_FETCH=1",
        "GIT_TERMINAL_PROMPT=0",
        "GIT_OPTIONAL_LOCKS=0",
        "GIT_ALLOW_PROTOCOL=",
        "GIT_NO_REPLACE_OBJECTS=1",
        "LC_ALL=C",
        "GIT_INDEX_FILE=/tmp/polygloss-test-index",
    ] {
        assert!(env.contains(&want), "missing {want} in {env:?}");
    }
    for gone in [
        "GIT_DIR=",
        "GIT_WORK_TREE=",
        "GIT_OBJECT_DIRECTORY=",
        "GIT_COMMON_DIR=",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES=",
        "GIT_CONFIG_PARAMETERS=",
        "GIT_CONFIG_COUNT=",
    ] {
        assert!(
            !env.iter().any(|l| l.starts_with(gone)),
            "{gone} leaked into {env:?}"
        );
    }
}

#[test]
fn git_binary_bypasses_the_xcode_shim() {
    // T2.10.1: the Xcode shim (`/usr/bin/git`) costs ~9 ms per process on top
    // of git itself; the runner spawns the git it would run instead.
    let _sb = Sandbox::isolate();
    let first_on_path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("git"))
        .find(|g| g.is_file());
    let started = std::time::Instant::now();
    let bin = git_binary();
    let lookup = started.elapsed();
    if cfg!(target_os = "macos") && first_on_path.as_deref() == Some(Path::new("/usr/bin/git")) {
        // A fresh HOME (every perf run, every sandbox) must not make the
        // lookup slow: `xcrun --find git` takes ~75 ms there.
        assert!(lookup < std::time::Duration::from_millis(40), "{lookup:?}");
        assert_ne!(bin, PathBuf::from("git"));
        assert_ne!(bin, PathBuf::from("/usr/bin/git"));
        assert!(bin.is_absolute() && bin.is_file(), "{bin:?}");
    } else {
        // Any other git (Homebrew on CI) is found on PATH by the spawn.
        assert_eq!(bin, PathBuf::from("git"));
    }
    // It is a working git that passes the version check.
    assert!(check_version().unwrap() >= GitVersion::MIN);
}

#[test]
fn sandbox_keeps_caller_git_bin() {
    // S13: `POLYGLOSS_GIT_BIN=<old git> nextest run` must reach the runner, so the
    // suite really runs against that git. Set before `isolate()`, like a caller does.
    let chosen = std::env::temp_dir().join("polygloss-chosen-git");
    set_env("POLYGLOSS_GIT_BIN", &chosen);
    let _sb = Sandbox::isolate();
    assert_eq!(git_binary(), chosen);
    assert_eq!(
        std::env::var_os("POLYGLOSS_GIT_BIN").as_deref(),
        Some(chosen.as_os_str())
    );
}

#[test]
fn git_runner_stays_offline_despite_inherited_overrides() {
    let sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    let url = format!("file://{}", repo.path().display());

    // The user's global config may allow a transport outright ...
    std::fs::write(
        sb.git_config_global(),
        "[protocol \"file\"]\n\tallow = always\n",
    )
    .unwrap();
    // ... and agents, hooks and `git -c` parents export config and protocol env.
    set_env("GIT_ALLOW_PROTOCOL", "file");
    set_env(
        "GIT_CONFIG_PARAMETERS",
        "'protocol.allow'='always' 'polygloss.fromparams'='yes'",
    );
    set_env("GIT_CONFIG_COUNT", "2");
    set_env("GIT_CONFIG_KEY_0", "protocol.file.allow");
    set_env("GIT_CONFIG_VALUE_0", "always");
    set_env("GIT_CONFIG_KEY_1", "polygloss.fromcount");
    set_env("GIT_CONFIG_VALUE_1", "yes");

    let git = Git::new(repo.path());
    match git.output(&os(&["ls-remote", &url])) {
        Err(GitError::Failed { stderr, .. }) => {
            assert!(stderr.contains("not allowed"), "{stderr}");
        }
        other => panic!("expected a refused transport, got {other:?}"),
    }
    // Inherited per-invocation config never reaches git ...
    for key in ["polygloss.fromparams", "polygloss.fromcount"] {
        assert_eq!(
            git.status(&os(&["config", "--get", key])).unwrap(),
            1,
            "{key}"
        );
    }
    // ... while the user's global config stays enabled (design §6.2).
    let global = git
        .output(&os(&["config", "--get", "protocol.file.allow"]))
        .unwrap();
    assert_eq!(text(global), "always");
}

#[test]
fn git_runner_ignores_replace_refs() {
    let _sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    repo.write("a.txt", b"two\n");
    let c2 = repo.commit("c2");
    let c2_tree = tree(&repo, "HEAD");
    let c1_tree = tree(&repo, "HEAD~1");
    // `refs/replace/<c2>` swaps in c1's content: every replace-aware git command
    // would now report c1's tree for c2.
    repo.git(&["replace", c2.as_str(), c1.as_str()]);
    assert_eq!(tree(&repo, c2.as_str()), c1_tree, "fixture git honours it");

    let r = resolve_in(
        &repo,
        Source::Commit {
            rev: c2.to_string(),
        },
    )
    .unwrap();
    assert_eq!(head_side(&r.head).tree, c2_tree);
    assert_eq!(r.base.tree, c1_tree);
    assert_eq!(r.base.commit.as_ref(), Some(&c1));
}

#[test]
fn git_runner_output_stdin_and_status() {
    let _sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    let git = Git::new(repo.path());

    let blob = text(
        git.output_stdin(&os(&["hash-object", "--stdin"]), b"one\n")
            .unwrap(),
    );
    assert_eq!(blob, repo.oid("HEAD:a.txt").as_str());

    let checked = text(
        git.output_stdin(
            &os(&["cat-file", "--batch-check=%(objectname) %(objecttype)"]),
            format!("{c1}\n").as_bytes(),
        )
        .unwrap(),
    );
    assert_eq!(checked, format!("{c1} commit"));

    // `status` reports exit codes without mapping them to errors.
    assert_eq!(git.status(&os(&["symbolic-ref", "-q", "HEAD"])).unwrap(), 0);
    repo.git(&["checkout", "-q", "--detach"]);
    assert_eq!(git.status(&os(&["symbolic-ref", "-q", "HEAD"])).unwrap(), 1);

    // `output` maps a non-zero exit to `Failed` with stderr.
    match git.output(&os(&["rev-parse", "--verify", "no-such-rev"])) {
        Err(GitError::Failed { code, stderr, .. }) => {
            assert_eq!(code, Some(128));
            assert!(stderr.contains("Needed a single revision"), "{stderr}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn git_spawn_counter_counts_by_subcommand() {
    let _sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    let git = Git::new(repo.path());
    let before = git_spawns("rev-parse");
    git.output(&os(&["rev-parse", "HEAD"])).unwrap();
    git.status(&os(&["rev-parse", "HEAD"])).unwrap();
    assert_eq!(git_spawns("rev-parse"), before + 2);
    assert_eq!(git_spawns("diff-tree"), 0);
}

// ---------------------------------------------------------------- version

#[test]
fn git_version_parses_apple_suffix() {
    let v = GitVersion::parse("git version 2.54.0 (Apple Git-157)\n").unwrap();
    assert_eq!(
        v,
        GitVersion {
            major: 2,
            minor: 54,
            patch: 0
        }
    );
    assert_eq!(v.to_string(), "2.54.0");
    assert_eq!(
        GitVersion::parse("git version 2.39.5").unwrap(),
        GitVersion {
            major: 2,
            minor: 39,
            patch: 5
        }
    );
    assert_eq!(
        GitVersion::parse("git version 2.45.1.windows.1").unwrap(),
        GitVersion {
            major: 2,
            minor: 45,
            patch: 1
        }
    );
    assert_eq!(
        GitVersion::parse("git version 2.50.0.rc1").unwrap(),
        GitVersion {
            major: 2,
            minor: 50,
            patch: 0
        }
    );
    assert!(GitVersion::parse("hub version 2.14.2").is_err());
    assert!(GitVersion::parse("git version banana").is_err());
}

#[test]
fn git_version_too_old_is_rejected() {
    let sb = Sandbox::isolate();

    // The real system git passes (the machine and CI run >= 2.39).
    let real = check_version().unwrap();
    assert!(real >= GitVersion::MIN, "{real}");

    let fake_dir = sb.home().join("fake");
    std::fs::create_dir_all(&fake_dir).unwrap();
    set_env(
        "POLYGLOSS_GIT_BIN",
        fake_git(&fake_dir, "git version 2.38.1"),
    );
    match check_version() {
        Err(GitError::TooOld { found, min }) => {
            assert_eq!(found.to_string(), "2.38.1");
            assert_eq!(min, GitVersion::MIN);
        }
        other => panic!("expected TooOld, got {other:?}"),
    }
    let msg = check_version().unwrap_err().to_string();
    assert!(msg.contains("2.38.1") && msg.contains("2.39.0"), "{msg}");

    set_env(
        "POLYGLOSS_GIT_BIN",
        fake_git(&fake_dir, "git version 2.39.0"),
    );
    assert_eq!(check_version().unwrap().to_string(), "2.39.0");

    set_env("POLYGLOSS_GIT_BIN", sb.home().join("no-such-git"));
    assert!(matches!(check_version(), Err(GitError::Spawn { .. })));
}

// ---------------------------------------------------------------- discovery

#[test]
fn discover_from_subdirectory() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha256);
    repo.write("src/deep/lib.rs", b"fn main() {}\n");
    repo.commit("c1");

    let info = discover(&repo.path().join("src/deep")).unwrap();
    assert_eq!(info.common_dir, repo.path().join(".git"));
    assert_eq!(info.git_dir, repo.path().join(".git"));
    assert_eq!(info.toplevel.as_deref(), Some(repo.path()));
    assert_eq!(info.object_format, ObjectFormat::Sha256);

    // Paths are realpath'd: a symlinked route to the same repo gives the same info.
    let link = sb.home().join("link-to-repo");
    std::os::unix::fs::symlink(repo.path(), &link).unwrap();
    assert_eq!(discover(&link.join("src")).unwrap(), info);

    // A file inside the worktree works too.
    assert_eq!(
        discover(&repo.path().join("src/deep/lib.rs")).unwrap(),
        info
    );

    let outside = sb.home().join("not-a-repo");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(matches!(discover(&outside), Err(GitError::NotARepo(_))));
    assert!(matches!(
        discover(&sb.home().join("missing")),
        Err(GitError::NotARepo(_))
    ));
}

#[test]
fn discover_linked_worktree_shares_common_dir() {
    let _sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    let wt = repo.add_worktree("feature");

    let main = discover(repo.path()).unwrap();
    let linked = discover(&wt).unwrap();
    assert_eq!(linked.common_dir, main.common_dir);
    assert_ne!(linked.git_dir, main.git_dir);
    assert!(linked.git_dir.starts_with(&main.common_dir));
    assert_eq!(linked.toplevel.as_deref(), Some(wt.as_path()));
    assert_eq!(linked.object_format, ObjectFormat::Sha1);

    // A bare repo has no toplevel.
    let bare = discover(&main.common_dir).unwrap();
    assert_eq!(bare.common_dir, main.common_dir);
    assert_eq!(bare.toplevel, None);
}

#[test]
fn discover_repo_path_with_newline() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("line one\nline two\n");
    Git::new(sb.home())
        .output(&[
            OsStr::new("init"),
            OsStr::new("-q"),
            OsStr::new("-b"),
            OsStr::new("main"),
            dir.as_os_str(),
        ])
        .unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();

    let info = discover(&dir.join("sub")).unwrap();
    assert_eq!(info.common_dir, dir.join(".git"));
    assert_eq!(info.git_dir, dir.join(".git"));
    assert_eq!(info.toplevel.as_deref(), Some(dir.as_path()));
    assert_eq!(info.object_format, ObjectFormat::Sha1);

    let r = resolve(
        &info,
        &dir.join("sub"),
        &Source::Live { since: Since::Head },
    )
    .unwrap();
    assert_eq!(
        r.review_key,
        format!("worktree:{}@main#since=HEAD", dir.display())
    );
}

// ---------------------------------------------------------------- commit sources

#[test]
fn resolve_commit_uses_first_parent_tree() {
    let _sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    repo.write("a.txt", b"two\n");
    let c2 = repo.commit("c2");

    let r = resolve_in(&repo, Source::Commit { rev: "HEAD".into() }).unwrap();
    assert_eq!(r.object_format, ObjectFormat::Sha1);
    assert_eq!(r.kind, ReviewKind::Commit);
    assert_eq!(r.base.tree, tree(&repo, "HEAD~1"));
    assert_eq!(r.base.commit, Some(c1));
    let head = head_side(&r.head);
    assert_eq!(head.tree, tree(&repo, "HEAD"));
    assert_eq!(head.commit.as_ref(), Some(&c2));
    assert_eq!(head.ref_name.as_deref(), Some("refs/heads/main"));
    assert_eq!(r.review_key, format!("commit:{c2}"));
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn resolve_root_commit_uses_empty_tree() {
    let _sb = Sandbox::isolate();
    for fmt in [ObjectFormat::Sha1, ObjectFormat::Sha256] {
        let (repo, c1) = one_commit(fmt);
        let r = resolve_in(&repo, Source::Commit { rev: "main".into() }).unwrap();
        assert_eq!(r.object_format, fmt);
        assert_eq!(r.base.tree, fmt.empty_tree());
        assert_eq!(r.base.commit, None);
        assert_eq!(head_side(&r.head).commit.as_ref(), Some(&c1));
        assert_eq!(head_side(&r.head).tree, tree(&repo, "main"));
    }
}

#[test]
fn resolve_merge_commit_uses_first_parent() {
    let _sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    repo.branch("feature");
    repo.write("main.txt", b"main\n");
    let c_main = repo.commit("main work");
    repo.checkout("feature");
    repo.write("feature.txt", b"feature\n");
    repo.commit("feature work");
    repo.checkout("main");
    repo.git(&["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);

    let r = resolve_in(&repo, Source::Commit { rev: "HEAD".into() }).unwrap();
    assert_eq!(r.base.commit, Some(c_main));
    assert_eq!(r.base.tree, tree(&repo, "HEAD^1"));
    assert_ne!(r.base.tree, tree(&repo, "HEAD^2"));
    assert_eq!(head_side(&r.head).tree, tree(&repo, "HEAD"));
}

// ---------------------------------------------------------------- compare sources

/// `main`: c1 → c2; `feature` (from c1): c3. Returns (c1, c2, c3).
fn forked() -> (FixtureRepo, Oid, Oid, Oid) {
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    repo.branch("feature");
    repo.write("main.txt", b"main\n");
    let c2 = repo.commit("c2");
    repo.checkout("feature");
    repo.write("feature.txt", b"feature\n");
    let c3 = repo.commit("c3");
    (repo, c1, c2, c3)
}

fn compare(base: &str, head: &str, mode: CompareMode) -> Source {
    Source::Compare {
        base: base.into(),
        head: head.into(),
        mode,
    }
}

#[test]
fn resolve_three_dot_uses_merge_base() {
    let _sb = Sandbox::isolate();
    let (repo, c1, _c2, c3) = forked();
    let r = resolve_in(&repo, compare("main", "feature", CompareMode::ThreeDot)).unwrap();
    assert_eq!(r.kind, ReviewKind::Compare);
    assert_eq!(r.base.tree, tree(&repo, &c1.to_string()));
    assert_eq!(r.base.commit, Some(c1));
    assert_eq!(r.base.ref_name.as_deref(), Some("refs/heads/main"));
    let head = head_side(&r.head);
    assert_eq!(head.commit.as_ref(), Some(&c3));
    assert_eq!(head.tree, tree(&repo, "feature"));
    assert_eq!(r.review_key, "compare:refs/heads/main...refs/heads/feature");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn resolve_three_dot_multiple_merge_bases_warns() {
    let _sb = Sandbox::isolate();
    // Criss-cross: x and y each merge the other's first commit.
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    repo.branch("x");
    repo.branch("y");
    repo.checkout("x");
    repo.write("x.txt", b"x\n");
    let x1 = repo.commit("x1");
    repo.checkout("y");
    repo.write("y.txt", b"y\n");
    let y1 = repo.commit("y1");
    repo.checkout("x");
    repo.git(&["merge", "-q", "--no-ff", "-m", "x merges y1", y1.as_str()]);
    repo.checkout("y");
    repo.git(&["merge", "-q", "--no-ff", "-m", "y merges x1", x1.as_str()]);

    let r = resolve_in(&repo, compare("x", "y", CompareMode::ThreeDot)).unwrap();
    let first = repo.git(&["merge-base", "x", "y"]);
    assert_eq!(
        r.base.commit.as_ref().map(Oid::as_str),
        Some(first.as_str())
    );
    assert_eq!(
        r.warnings,
        [ResolveWarning::MultipleMergeBases {
            count: 2,
            used: r.base.commit.clone().unwrap()
        }]
    );
}

#[test]
fn resolve_direct_uses_base_tree() {
    let _sb = Sandbox::isolate();
    let (repo, _c1, c2, c3) = forked();
    let r = resolve_in(&repo, compare("main", "feature", CompareMode::Direct)).unwrap();
    assert_eq!(r.base.tree, tree(&repo, "main"));
    assert_eq!(r.base.commit, Some(c2));
    assert_eq!(head_side(&r.head).commit, Some(c3));
    assert_eq!(r.review_key, "compare:refs/heads/main..refs/heads/feature");
}

#[test]
fn resolve_no_merge_base_suggests_direct() {
    let _sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    repo.git(&["checkout", "-q", "--orphan", "unrelated"]);
    repo.write("other.txt", b"unrelated\n");
    repo.commit("unrelated root");

    let err = resolve_in(&repo, compare("main", "unrelated", CompareMode::ThreeDot)).unwrap_err();
    match &err {
        ResolveError::NoMergeBase { suggestion } => assert!(suggestion.contains("--direct")),
        other => panic!("expected NoMergeBase, got {other:?}"),
    }
    assert!(err.to_string().contains("--direct"), "{err}");

    let direct = resolve_in(&repo, compare("main", "unrelated", CompareMode::Direct)).unwrap();
    assert_eq!(direct.base.tree, tree(&repo, "main"));
}

#[test]
fn resolve_ref_inputs_store_full_names() {
    let _sb = Sandbox::isolate();
    let (repo, c1, c2, _c3) = forked();
    repo.git(&["tag", "-a", "v1", "-m", "release", c1.as_str()]);
    repo.git(&["update-ref", "refs/remotes/origin/main", c2.as_str()]);

    let r = resolve_in(&repo, compare("v1", "origin/main", CompareMode::Direct)).unwrap();
    assert_eq!(
        r.review_key,
        "compare:refs/tags/v1..refs/remotes/origin/main"
    );
    assert_eq!(r.base.ref_name.as_deref(), Some("refs/tags/v1"));
    // Annotated tags peel to their commit.
    assert_eq!(r.base.commit.as_ref(), Some(&c1));
    let head = head_side(&r.head);
    assert_eq!(head.ref_name.as_deref(), Some("refs/remotes/origin/main"));
    assert_eq!(head.commit.as_ref(), Some(&c2));

    // HEAD on a branch stores the branch's full name.
    let r = resolve_in(&repo, compare("main", "HEAD", CompareMode::ThreeDot)).unwrap();
    assert_eq!(r.review_key, "compare:refs/heads/main...refs/heads/feature");
}

#[test]
fn resolve_oid_input_keys_by_commit() {
    let _sb = Sandbox::isolate();
    let (repo, c1, c2, c3) = forked();

    let r = resolve_in(
        &repo,
        compare(c1.short(), "feature~0", CompareMode::ThreeDot),
    )
    .unwrap();
    assert_eq!(r.review_key, format!("compare:{c1}...{c3}"));
    assert_eq!(r.base.ref_name, None);
    assert_eq!(head_side(&r.head).ref_name, None);

    let r = resolve_in(
        &repo,
        Source::Commit {
            rev: c2.short().into(),
        },
    )
    .unwrap();
    assert_eq!(r.review_key, format!("commit:{c2}"));
    assert_eq!(head_side(&r.head).ref_name, None);

    // A detached HEAD is not a ref name either.
    repo.git(&["checkout", "-q", "--detach", "main"]);
    let r = resolve_in(&repo, compare(c1.as_str(), "HEAD", CompareMode::Direct)).unwrap();
    assert_eq!(r.review_key, format!("compare:{c1}..{c2}"));
}

// ---------------------------------------------------------------- live sources

fn live(since: Since) -> Source {
    Source::Live { since }
}

#[test]
fn resolve_live_default_since_merge_base_with_origin_head() {
    let _sb = Sandbox::isolate();
    let (repo, c1, c2, _c3) = forked();
    repo.git(&["update-ref", "refs/remotes/origin/main", c2.as_str()]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repo.write("uncommitted.txt", b"dirty\n");

    let r = resolve_in(&repo, live(Since::MergeBase)).unwrap();
    assert_eq!(r.kind, ReviewKind::Live);
    assert_eq!(r.head, HeadSpec::Worktree);
    assert_eq!(r.base.commit.as_ref(), Some(&c1));
    assert_eq!(r.base.tree, tree(&repo, c1.as_str()));
    assert_eq!(r.base.ref_name.as_deref(), Some("refs/remotes/origin/main"));
    assert_eq!(
        r.review_key,
        format!(
            "worktree:{}@feature#since=merge-base",
            repo.path().display()
        )
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    // From a subdirectory the key still names the worktree root.
    repo.write("sub/dir/f.txt", b"f\n");
    let info = discover(repo.path()).unwrap();
    let from_sub = resolve(&info, &repo.path().join("sub/dir"), &live(Since::MergeBase)).unwrap();
    assert_eq!(from_sub.review_key, r.review_key);

    // since=HEAD and since=<commit> are their own keys.
    let head = resolve_in(&repo, live(Since::Head)).unwrap();
    assert_eq!(head.base.tree, tree(&repo, "HEAD"));
    assert_eq!(
        head.review_key,
        format!("worktree:{}@feature#since=HEAD", repo.path().display())
    );
    let fixed = resolve_in(&repo, live(Since::Commit("main".into()))).unwrap();
    assert_eq!(fixed.base.commit.as_ref(), Some(&c2));
    assert_eq!(
        fixed.review_key,
        format!("worktree:{}@feature#since={c2}", repo.path().display())
    );
}

#[test]
fn default_branch_fallback_chain() {
    let _sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    let git = Git::new(repo.path());
    let c = c1.as_str();
    for r in ["origin/trunk", "origin/main", "origin/master"] {
        repo.git(&["update-ref", &format!("refs/remotes/{r}"), c]);
    }
    repo.branch("master");
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);

    let step = || default_branch(&git).unwrap();
    assert_eq!(step(), ("refs/remotes/origin/trunk".to_owned(), false));

    repo.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    assert_eq!(step(), ("refs/remotes/origin/main".to_owned(), true));

    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    assert_eq!(step(), ("refs/remotes/origin/master".to_owned(), true));

    repo.git(&["update-ref", "-d", "refs/remotes/origin/master"]);
    assert_eq!(step(), ("refs/heads/main".to_owned(), true));

    repo.git(&["branch", "-q", "-m", "main", "work"]);
    assert_eq!(step(), ("refs/heads/master".to_owned(), true));

    repo.git(&["branch", "-q", "-D", "master"]);
    assert!(matches!(
        default_branch(&git),
        Err(GitError::NoDefaultBranch)
    ));

    // A dangling origin/HEAD falls through the chain too.
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/gone",
    ]);
    repo.git(&["update-ref", "refs/remotes/origin/master", c]);
    assert_eq!(step(), ("refs/remotes/origin/master".to_owned(), true));
}

#[test]
fn resolve_live_in_unborn_repo_uses_empty_tree() {
    let _sb = Sandbox::isolate();
    for fmt in [ObjectFormat::Sha1, ObjectFormat::Sha256] {
        let repo = FixtureRepo::init(fmt);
        repo.write("new.txt", b"not committed yet\n");

        let r = resolve_in(&repo, live(Since::MergeBase)).unwrap();
        assert_eq!(r.object_format, fmt);
        assert_eq!(r.base.tree, fmt.empty_tree());
        assert_eq!(r.base.commit, None);
        assert_eq!(r.head, HeadSpec::Worktree);
        assert_eq!(
            r.review_key,
            format!("worktree:{}@main#since=merge-base", repo.path().display())
        );
        assert_eq!(r.warnings, [ResolveWarning::UnbornHead]);

        let r = resolve_in(&repo, live(Since::Head)).unwrap();
        assert_eq!(r.base.tree, fmt.empty_tree());
        assert_eq!(
            r.review_key,
            format!("worktree:{}@main#since=HEAD", repo.path().display())
        );
    }
}

#[test]
fn resolve_live_without_merge_base_falls_back_to_head_with_notice() {
    let _sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    repo.git(&["branch", "-q", "-m", "main", "work"]);
    let key = format!("worktree:{}@work#since=merge-base", repo.path().display());

    // No default branch anywhere in the OQ-5 chain.
    let r = resolve_in(&repo, live(Since::MergeBase)).unwrap();
    assert_eq!(r.base.commit.as_ref(), Some(&c1));
    assert_eq!(r.base.tree, tree(&repo, "HEAD"));
    assert_eq!(r.review_key, key);
    assert_eq!(r.warnings, [ResolveWarning::NoDefaultBranch]);
    assert!(r.warnings[0].to_string().contains("HEAD"));

    // A default branch with no common history.
    repo.git(&["checkout", "-q", "--orphan", "elsewhere"]);
    repo.write("elsewhere.txt", b"x\n");
    let other = repo.commit("unrelated root");
    repo.checkout("work");
    repo.git(&["update-ref", "refs/remotes/origin/main", other.as_str()]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let r = resolve_in(&repo, live(Since::MergeBase)).unwrap();
    assert_eq!(r.base.commit.as_ref(), Some(&c1));
    assert_eq!(r.review_key, key);
    assert_eq!(
        r.warnings,
        [ResolveWarning::NoMergeBase {
            default_branch: "refs/remotes/origin/main".into()
        }]
    );
}

#[test]
fn resolve_detached_head_live_key() {
    let _sb = Sandbox::isolate();
    let (repo, c1) = one_commit(ObjectFormat::Sha1);
    repo.write("a.txt", b"two\n");
    let c2 = repo.commit("c2");
    repo.git(&["checkout", "-q", "--detach"]);

    let r = resolve_in(&repo, live(Since::Head)).unwrap();
    assert_eq!(
        r.review_key,
        format!("worktree:{}@detached#since=HEAD", repo.path().display())
    );
    assert_eq!(r.base.commit.as_ref(), Some(&c2));
    assert_eq!(r.base.ref_name, None);

    let r = resolve_in(&repo, live(Since::Commit("HEAD~1".into()))).unwrap();
    assert_eq!(
        r.review_key,
        format!("worktree:{}@detached#since={c1}", repo.path().display())
    );
    assert_eq!(r.base.tree, tree(&repo, "HEAD~1"));
}

// ---------------------------------------------------------------- hostile inputs

#[test]
fn resolve_shallow_missing_objects_errors() {
    let _sb = Sandbox::isolate();
    let (source, c1) = one_commit(ObjectFormat::Sha1);
    source.branch("feature");
    source.write("a.txt", b"two\n");
    source.commit("c2");
    source.write("a.txt", b"three\n");
    source.commit("c3");
    source.checkout("feature");
    source.write("f.txt", b"feature\n");
    source.commit("c4");
    source.checkout("main");

    let shallow = source.clone_shallow();
    shallow.git(&["fetch", "-q", "--depth", "1", "origin", "feature"]);
    shallow.git(&["update-ref", "refs/heads/feature", "FETCH_HEAD"]);
    let objects_before = shallow.git(&["count-objects", "-v"]);

    // HEAD's parent is beyond the shallow boundary: never pretend it is a root commit.
    match resolve_in(&shallow, Source::Commit { rev: "HEAD".into() }) {
        Err(ResolveError::ObjectsMissing(msg)) => assert!(!msg.is_empty()),
        other => panic!("expected ObjectsMissing, got {other:?}"),
    }
    // The merge base (c1) was never fetched: an error, not "no merge base".
    match resolve_in(&shallow, compare("main", "feature", CompareMode::ThreeDot)) {
        Err(ResolveError::ObjectsMissing(_)) => {}
        other => panic!("expected ObjectsMissing, got {other:?}"),
    }
    // A full commit id the clone does not have is missing history, not a typo, and
    // it is not fetched either.
    for source in [
        Source::Commit {
            rev: c1.to_string(),
        },
        compare(c1.as_str(), "main", CompareMode::Direct),
        live(Since::Commit(c1.to_string())),
    ] {
        match resolve_in(&shallow, source.clone()) {
            Err(ResolveError::ObjectsMissing(msg)) => {
                assert!(msg.contains(c1.as_str()), "{msg}");
            }
            other => panic!("{source:?}: expected ObjectsMissing, got {other:?}"),
        }
    }
    // Names that do not resolve, and ids of present non-commits, stay BadRevision.
    let blob = shallow.oid("HEAD:a.txt");
    for rev in ["no-such-branch", c1.short(), blob.as_str()] {
        let err = resolve_in(&shallow, Source::Commit { rev: rev.into() }).unwrap_err();
        assert!(
            matches!(err, ResolveError::BadRevision(_)),
            "{rev}: {err:?}"
        );
    }

    assert_eq!(shallow.git(&["count-objects", "-v"]), objects_before);

    // Direct compare between present commits still works.
    resolve_in(&shallow, compare("main", "feature", CompareMode::Direct)).unwrap();
}

#[test]
fn resolve_live_in_shallow_clone_blames_missing_history() {
    let _sb = Sandbox::isolate();
    let (source, _c1) = one_commit(ObjectFormat::Sha1);
    source.branch("feature");
    source.write("a.txt", b"two\n");
    source.commit("c2");
    source.checkout("feature");
    source.write("f.txt", b"feature\n");
    let c3 = source.commit("c3");
    source.checkout("main");

    // A depth-1 clone of main plus a depth-1 fetch of feature: the real merge base
    // (c1) is outside the clone, so git finds none.
    let shallow = source.clone_shallow();
    shallow.git(&["fetch", "-q", "--depth", "1", "origin", "feature"]);
    shallow.git(&["checkout", "-q", "-b", "feature", "FETCH_HEAD"]);
    assert_eq!(shallow.oid("HEAD"), c3);

    let r = resolve_in(&shallow, live(Since::MergeBase)).unwrap();
    assert_eq!(r.base.commit.as_ref(), Some(&c3));
    assert_eq!(
        r.review_key,
        format!(
            "worktree:{}@feature#since=merge-base",
            shallow.path().display()
        )
    );
    assert_eq!(
        r.warnings,
        [ResolveWarning::MergeBaseBeyondShallow {
            default_branch: "refs/remotes/origin/main".into()
        }]
    );
    let notice = r.warnings[0].to_string();
    assert!(
        notice.contains("shallow") && notice.contains("HEAD"),
        "{notice}"
    );
}

/// Objects git reports missing in `repo`, listed without fetching anything.
fn missing_objects(repo: &FixtureRepo) -> Vec<String> {
    let out = repo.git(&["rev-list", "--objects", "--all", "--missing=print"]);
    out.lines()
        .filter_map(|l| l.strip_prefix('?'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn resolve_blobless_clone_never_fetches() {
    let sb = Sandbox::isolate();
    let (source, c1) = one_commit(ObjectFormat::Sha1);
    let old_blob = source.oid("HEAD:a.txt");
    source.write("a.txt", b"two\n");
    let c2 = source.commit("c2");

    let partial = source.clone_blobless();
    assert_eq!(
        partial.git(&["config", "--get", "remote.origin.promisor"]),
        "true"
    );
    let missing = missing_objects(&partial);
    assert_eq!(missing, [old_blob.to_string()]);

    // Even with the transport re-enabled by the user's config and inherited env.
    std::fs::write(
        sb.git_config_global(),
        "[protocol]\n\tallow = always\n[protocol \"file\"]\n\tallow = always\n",
    )
    .unwrap();
    set_env("GIT_ALLOW_PROTOCOL", "file");

    // Commits and trees are all there: resolving HEAD~1 works.
    let r = resolve_in(&partial, Source::Commit { rev: "HEAD".into() }).unwrap();
    assert_eq!(r.base.commit.as_ref(), Some(&c1));
    assert_eq!(head_side(&r.head).commit.as_ref(), Some(&c2));

    // Reading the missing blob through the runner fails instead of fetching it.
    let git = Git::new(partial.path());
    let out = git
        .run(&os(&["cat-file", "-p", old_blob.as_str()]))
        .unwrap();
    assert!(!out.success(), "{out:?}");

    // A commit made upstream after the clone is missing history, not a bad name.
    source.write("a.txt", b"three\n");
    let c3 = source.commit("c3");
    let err = resolve_in(
        &partial,
        Source::Commit {
            rev: c3.to_string(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, ResolveError::ObjectsMissing(_)), "{err:?}");

    assert_eq!(missing_objects(&partial), missing);
}

#[test]
fn rev_starting_with_dash_is_not_an_option() {
    let sb = Sandbox::isolate();
    let (repo, _) = one_commit(ObjectFormat::Sha1);
    let pwned = sb.home().join("pwned");
    let evil = format!("--output={}", pwned.display());

    for source in [
        Source::Commit { rev: evil.clone() },
        compare("-h", "main", CompareMode::ThreeDot),
        compare("main", &evil, CompareMode::Direct),
        live(Since::Commit("--all".into())),
    ] {
        match resolve_in(&repo, source.clone()) {
            Err(ResolveError::BadRevision(rev)) => assert!(rev.starts_with('-'), "{rev}"),
            other => panic!("{source:?}: expected BadRevision, got {other:?}"),
        }
    }
    assert!(!pwned.exists());

    let err = resolve_in(
        &repo,
        Source::Commit {
            rev: "no-such-branch".into(),
        },
    )
    .unwrap_err();
    assert!(
        matches!(&err, ResolveError::BadRevision(rev) if rev == "no-such-branch"),
        "{err:?}"
    );
    assert!(err.to_string().contains("no-such-branch"));
}
