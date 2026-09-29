//! Worktree snapshots in a scratch store, pinning and pruning (T1.5, design §5,
//! ADR-0008). Every test runs under `Sandbox::isolate()`; the scratch root lives in
//! the sandbox cache dir, never in the real `~/Library/Caches`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use polygloss_core::git::{
    Git, LiveState, RepoInfo, SnapshotError, Snapshotter, discover, list_changes, snapshot_ref,
};
use polygloss_core::objects::{BlobReader, ObjectError};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::{ObjectFormat, Oid};

fn snapshotter(sb: &Sandbox) -> Snapshotter {
    Snapshotter::new(sb.cache_dir().join("polygloss").join("scratch"))
}

fn info(repo: &FixtureRepo) -> RepoInfo {
    discover(repo.path()).unwrap()
}

fn fmt_of(repo: &FixtureRepo) -> ObjectFormat {
    info(repo).object_format
}

/// A repo with one commit: `a.txt`, `dir/b.txt` and a `.gitignore` for `*.log`.
fn base_repo(fmt: ObjectFormat) -> FixtureRepo {
    let repo = FixtureRepo::init(fmt);
    repo.write("a.txt", b"alpha\n");
    repo.write("dir/b.txt", b"bravo\n");
    repo.write(".gitignore", b"*.log\n");
    repo.commit("c1");
    repo
}

/// Paths in `tree` (recursive), read with plain git in the fixture repo plus the
/// snapshot's scratch store.
fn ls_tree(repo: &FixtureRepo, state: &LiveState) -> Vec<String> {
    let out = Command::new("git")
        .current_dir(repo.path())
        .env("GIT_OBJECT_DIRECTORY", &state.scratch_objects)
        .args([
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            state.head_tree.as_str(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8(p.to_vec()).unwrap())
        .collect()
}

/// The blob id of `path` inside the snapshot tree.
fn blob_in(repo: &FixtureRepo, state: &LiveState, path: &str) -> Oid {
    let out = Command::new("git")
        .current_dir(repo.path())
        .env("GIT_OBJECT_DIRECTORY", &state.scratch_objects)
        .args(["rev-parse", &format!("{}:{path}", state.head_tree)])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Oid::parse(String::from_utf8(out.stdout).unwrap().trim(), fmt_of(repo)).unwrap()
}

/// Every file (with its bytes), directory and socket under `dir`, to prove a
/// directory was not written.
/// (Not mtimes: when `git add` finds an object it would write already present in
/// an alternate it "freshens" that file's mtime, and it freshens a split index's
/// `sharedindex.*` on read. Contents and the set of files never change.)
fn listing(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                out.insert(entry.path(), b"<dir>".to_vec());
                walk(&entry.path(), out);
            } else if kind.is_file() {
                out.insert(entry.path(), std::fs::read(entry.path()).unwrap());
            } else {
                out.insert(entry.path(), b"<special>".to_vec());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

/// Whether the repo itself has `oid` (never lazily fetched in a partial clone).
fn cat_file_exists(repo: &FixtureRepo, oid: &Oid) -> bool {
    Command::new("git")
        .current_dir(repo.path())
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["cat-file", "-e", oid.as_str()])
        .status()
        .unwrap()
        .success()
}

#[test]
fn snapshot_includes_untracked_excludes_ignored() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"alpha changed\n");
    repo.write("new/untracked.txt", b"new\n");
    repo.write("debug.log", b"ignored\n");
    std::fs::remove_file(repo.path().join("dir/b.txt")).unwrap();

    let state = snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();

    assert_eq!(
        ls_tree(&repo, &state),
        [".gitignore", "a.txt", "new/untracked.txt"]
    );
    assert_eq!(state.worktree, repo.path());
    assert!(state.scratch_objects.starts_with(sb.cache_dir()));
    let blobs = BlobReader::open(&info(&repo))
        .unwrap()
        .with_scratch(&state.scratch_objects)
        .unwrap();
    assert_eq!(
        &*blobs.read(&blob_in(&repo, &state, "a.txt")).unwrap(),
        b"alpha changed\n"
    );
}

#[test]
fn snapshot_leaves_user_index_untouched() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    // A partially staged state: staged edit, then a further unstaged edit, plus an
    // untracked file.
    repo.write("a.txt", b"staged\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", b"staged then edited\n");
    repo.write("untracked.txt", b"u\n");
    let index = repo.path().join(".git/index");
    let index_before = std::fs::read(&index).unwrap();
    let status_before = repo.git(&["status", "--porcelain=v2", "-z"]);
    let repo_info = info(&repo);

    // What agents and hooks export (RF3), all pointing somewhere else: the snapshot
    // must neither follow nor write through them.
    let other = base_repo(ObjectFormat::Sha1);
    let other_git_dir = other.path().join(".git");
    let other_before = listing(&other_git_dir);
    let bogus = sb.home().join("bogus");
    std::fs::create_dir_all(&bogus).unwrap();
    let exported = [
        ("GIT_DIR", other_git_dir.clone()),
        ("GIT_WORK_TREE", other.path().to_path_buf()),
        ("GIT_COMMON_DIR", other_git_dir.clone()),
        ("GIT_INDEX_FILE", bogus.join("index")),
        ("GIT_OBJECT_DIRECTORY", bogus.join("objects")),
        ("GIT_ALTERNATE_OBJECT_DIRECTORIES", bogus.join("alt")),
    ];
    for (key, val) in &exported {
        set_env(key, val);
    }
    let state = snapshotter(&sb).snapshot(&repo_info, repo.path());
    for (key, _) in &exported {
        remove_env(key);
    }
    let state = state.unwrap();

    assert_eq!(std::fs::read(&index).unwrap(), index_before);
    assert_eq!(repo.git(&["status", "--porcelain=v2", "-z"]), status_before);
    assert_eq!(
        ls_tree(&repo, &state),
        [".gitignore", "a.txt", "dir/b.txt", "untracked.txt"]
    );
    assert_eq!(listing(&other_git_dir), other_before);
    assert_eq!(std::fs::read_dir(&bogus).unwrap().count(), 0);
}

fn set_env(key: &str, val: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: one process per test (nextest); no other thread reads env here.
    unsafe { std::env::set_var(key, val) }
}

fn remove_env(key: &str) {
    // SAFETY: as in `set_env`.
    unsafe { std::env::remove_var(key) }
}

#[test]
fn snapshot_with_fsmonitor_configured_starts_no_daemon() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.git(&["config", "core.fsmonitor", "true"]);
    repo.write("a.txt", b"watched\n");
    let before = listing(&repo.path().join(".git"));
    let state = snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();
    // No daemon socket, cookie dir or anything else appears in the git dir.
    assert_eq!(listing(&repo.path().join(".git")), before);
    assert_eq!(ls_tree(&repo, &state), [".gitignore", "a.txt", "dir/b.txt"]);
}

#[test]
fn snapshot_writes_no_objects_into_repo() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"brand new content\n");
    repo.write("fresh/file.txt", b"fresh\n");
    let count_before = repo.git(&["count-objects", "-v"]);
    let git_dir_before = listing(&repo.path().join(".git"));

    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    // Diffing through the snapshot env reads both stores and writes nothing either.
    let git = snap.diff_env(&state, Git::new(repo.path()));
    let base = repo.oid("HEAD^{tree}");
    let changes = list_changes(&git, ObjectFormat::Sha1, &base, &state.head_tree).unwrap();
    assert_eq!(changes.len(), 2);

    assert_eq!(repo.git(&["count-objects", "-v"]), count_before);
    assert_eq!(listing(&repo.path().join(".git")), git_dir_before);
    assert!(!cat_file_exists(&repo, &state.head_tree));
}

#[test]
fn snapshot_tree_equals_real_add_all() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"changed\n");
    repo.write("x/y/z.txt", b"deep\n");
    repo.write("ignored.log", b"no\n");
    repo.write("exec.sh", b"#!/bin/sh\n");
    std::fs::set_permissions(
        repo.path().join("exec.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink("a.txt", repo.path().join("link")).unwrap();

    let state = snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();

    // The same thing done for real, on a copy of the worktree and repo.
    let copy_root = tempfile::tempdir().unwrap();
    let copy = copy_root.path().join("repo");
    let status = Command::new("cp")
        .args(["-Rp"])
        .arg(repo.path())
        .arg(&copy)
        .status()
        .unwrap();
    assert!(status.success());
    git_ok(&copy, &["add", "-A"]);
    let real = git_ok(&copy, &["write-tree"]);
    assert_eq!(state.head_tree.as_str(), real);
}

#[test]
fn snapshot_sha256_repo() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha256);
    repo.write("a.txt", b"sha256 change\n");
    let state = snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();
    assert_eq!(state.head_tree.object_format(), ObjectFormat::Sha256);
    assert_eq!(ls_tree(&repo, &state), [".gitignore", "a.txt", "dir/b.txt"]);
}

#[test]
fn snapshot_is_repeatable_and_unchanged_worktree_gives_same_tree() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let snap = snapshotter(&sb);
    // A clean worktree snapshots to HEAD's tree.
    let clean = snap.snapshot(&info(&repo), repo.path()).unwrap();
    assert_eq!(clean.head_tree, repo.oid("HEAD^{tree}"));
    repo.write("a.txt", b"v2\n");
    let one = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let two = snap.snapshot(&info(&repo), repo.path()).unwrap();
    assert_eq!(one, two);
    assert_ne!(one.head_tree, clean.head_tree);
}

#[test]
fn snapshot_from_subdirectory_uses_toplevel() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("new.txt", b"n\n");
    let sub = repo.path().join("dir");
    let state = snapshotter(&sb)
        .snapshot(&discover(&sub).unwrap(), &sub)
        .unwrap();
    assert_eq!(state.worktree, repo.path());
    assert!(ls_tree(&repo, &state).contains(&"new.txt".to_owned()));
}

#[test]
fn diff_env_lets_diff_tree_see_both_trees() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"alpha\nmore\n");
    repo.write("added.txt", b"added\n");
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let base = repo.oid("HEAD^{tree}");

    // Without the scratch env the snapshot tree is unknown to git.
    assert!(
        list_changes(
            &Git::new(repo.path()),
            ObjectFormat::Sha1,
            &base,
            &state.head_tree
        )
        .is_err()
    );

    let git = snap.diff_env(&state, Git::new(repo.path()));
    let changes = list_changes(&git, ObjectFormat::Sha1, &base, &state.head_tree).unwrap();
    let paths: Vec<&str> = changes.iter().map(|c| c.display_path()).collect();
    assert_eq!(paths, ["a.txt", "added.txt"]);
}

#[test]
fn snapshot_in_linked_worktree_uses_its_index() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let wt = repo.add_worktree("feature");
    // Both worktrees have an ignored file; only the linked worktree's index tracks it.
    std::fs::write(repo.path().join("keep.log"), b"main\n").unwrap();
    std::fs::write(wt.join("keep.log"), b"linked\n").unwrap();
    git_ok(&wt, &["add", "-f", "keep.log"]);
    let wt_repo = discover(&wt).unwrap();
    assert_eq!(wt_repo.common_dir, info(&repo).common_dir);

    let snap = snapshotter(&sb);
    let main_state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let wt_state = snap.snapshot(&wt_repo, &wt).unwrap();

    assert_eq!(wt_state.worktree, wt);
    assert!(!ls_tree(&repo, &main_state).contains(&"keep.log".to_owned()));
    assert!(ls_tree(&repo, &wt_state).contains(&"keep.log".to_owned()));
    // One shared object store per repo.
    assert_eq!(main_state.scratch_objects, wt_state.scratch_objects);
    // The linked worktree's own index is untouched too.
    let wt_index = wt_repo.git_dir.join("index");
    let before = std::fs::read(&wt_index).unwrap();
    snap.snapshot(&wt_repo, &wt).unwrap();
    assert_eq!(std::fs::read(&wt_index).unwrap(), before);
}

#[test]
fn snapshot_honors_global_excludes_file() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let excludes = sb.home().join("global-ignore");
    std::fs::write(&excludes, "*.secret\n").unwrap();
    std::fs::write(
        sb.git_config_global(),
        format!("[core]\n\texcludesFile = {}\n", excludes.display()),
    )
    .unwrap();
    repo.write("token.secret", b"s3cr3t\n");
    repo.write("visible.txt", b"v\n");

    let state = snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();
    let paths = ls_tree(&repo, &state);
    assert!(paths.contains(&"visible.txt".to_owned()));
    assert!(!paths.contains(&"token.secret".to_owned()));
}

#[test]
fn snapshot_unborn_repo() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let snap = snapshotter(&sb);
    let empty = snap.snapshot(&info(&repo), repo.path()).unwrap();
    assert_eq!(empty.head_tree, ObjectFormat::Sha1.empty_tree());
    repo.write("first.txt", b"1\n");
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    assert_eq!(ls_tree(&repo, &state), ["first.txt"]);
    // Still unborn, still no index.
    assert!(!repo.path().join(".git/index").exists());
}

#[test]
fn snapshot_returns_index_locked_when_lock_present() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, b"").unwrap();
    let result = snapshotter(&sb).snapshot(&info(&repo), repo.path());
    assert!(
        matches!(result, Err(SnapshotError::IndexLocked)),
        "{result:?}"
    );
    assert!(lock.exists(), "the user's lock is never removed");
    std::fs::remove_file(&lock).unwrap();
    snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();
}

#[test]
fn snapshot_with_split_index_writes_nothing_into_git_dir() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.git(&["config", "core.splitIndex", "true"]);
    repo.git(&["config", "splitIndex.maxPercentChange", "0"]);
    repo.git(&["update-index", "--split-index"]);
    // Enough changed entries that git would write a fresh shared index.
    repo.write("a.txt", b"split\n");
    for i in 0..20 {
        repo.write(&format!("s/{i}.txt"), b"s\n");
    }
    let before = listing(&repo.path().join(".git"));
    let state = snapshotter(&sb)
        .snapshot(&info(&repo), repo.path())
        .unwrap();
    assert_eq!(listing(&repo.path().join(".git")), before);
    assert_eq!(ls_tree(&repo, &state).len(), 23);
}

#[test]
fn snapshot_concurrent_same_worktree() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    for i in 0..50 {
        repo.write(&format!("many/{i}.txt"), format!("{i}\n").as_bytes());
    }
    let snap = snapshotter(&sb);
    let repo_info = info(&repo);
    let states: Vec<LiveState> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| s.spawn(|| snap.snapshot(&repo_info, repo.path()).unwrap()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(states.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn pin_copies_objects_and_survives_gc() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"pinned content\n");
    repo.write("n/new.txt", b"new pinned file\n");
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let blob = blob_in(&repo, &state, "n/new.txt");
    assert!(!cat_file_exists(&repo, &blob));

    let name = snap.pin(&info(&repo), &state).unwrap();
    assert_eq!(
        name,
        format!("refs/polygloss/snapshots/{}", state.head_tree)
    );
    assert_eq!(name, snapshot_ref(&state.head_tree));
    assert_eq!(repo.git(&["rev-parse", &name]), state.head_tree.as_str());

    // The repo alone must hold everything: drop the scratch store, then gc.
    std::fs::remove_dir_all(sb.cache_dir().join("polygloss")).unwrap();
    repo.git(&["gc", "-q", "--prune=now"]);
    assert!(cat_file_exists(&repo, &state.head_tree));
    assert!(cat_file_exists(&repo, &blob));
    let listed = repo.git(&["ls-tree", "-r", "--name-only", &name]);
    assert_eq!(listed, ".gitignore\na.txt\ndir/b.txt\nn/new.txt");
    assert_eq!(
        repo.git(&["cat-file", "-p", blob.as_str()]),
        "new pinned file"
    );
    // Pinning touched no branch, HEAD or index.
    assert_eq!(repo.git(&["status", "--porcelain"]), " M a.txt\n?? n/");
}

/// Writes `id` (read from the snapshot's scratch store) into the repo alone, as a
/// pin that stopped right after writing a tree, before its children, would.
fn plant_in_repo(repo: &FixtureRepo, state: &LiveState, id: &Oid) {
    let out = Command::new("git")
        .current_dir(repo.path())
        .env("GIT_OBJECT_DIRECTORY", &state.scratch_objects)
        .args(["cat-file", "tree", id.as_str()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), &out.stdout).unwrap();
    let written = repo.git(&[
        "hash-object",
        "-w",
        "-t",
        "tree",
        tmp.path().to_str().unwrap(),
    ]);
    assert_eq!(written, id.as_str());
}

#[test]
fn pin_completes_a_partially_copied_tree_and_gc_succeeds() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"partial pin\n");
    repo.write("n/deep/new.txt", b"only in the scratch store\n");
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let blob = blob_in(&repo, &state, "n/deep/new.txt");
    // Only the root tree made it into the repo; its new subtrees and blobs did not.
    plant_in_repo(&repo, &state, &state.head_tree);
    assert!(cat_file_exists(&repo, &state.head_tree));
    assert!(!cat_file_exists(&repo, &blob));

    let name = snap.pin(&info(&repo), &state).unwrap();

    std::fs::remove_dir_all(sb.cache_dir().join("polygloss")).unwrap();
    repo.git(&["gc", "-q", "--prune=now"]);
    assert!(cat_file_exists(&repo, &blob));
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &name]),
        ".gitignore\na.txt\ndir/b.txt\nn/deep/new.txt"
    );
    repo.git(&["fsck", "--connectivity-only", "--no-dangling"]);
}

#[test]
fn pin_errors_when_an_object_is_missing_from_both_stores() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("n/new.txt", b"about to be purged\n");
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let blob = blob_in(&repo, &state, "n/new.txt");
    // A damaged or partly purged scratch store (macOS may purge ~/Library/Caches).
    let loose = state
        .scratch_objects
        .join(&blob.as_str()[..2])
        .join(&blob.as_str()[2..]);
    std::fs::remove_file(loose).unwrap();

    let result = snap.pin(&info(&repo), &state);
    match result {
        Err(SnapshotError::Io(err)) => assert_eq!(err.kind(), std::io::ErrorKind::InvalidData),
        other => panic!("expected an InvalidData error, got {other:?}"),
    }
    assert!(
        repo.git(&["for-each-ref", &snapshot_ref(&state.head_tree)])
            .is_empty()
    );
    repo.git(&["gc", "-q", "--prune=now"]);
}

#[test]
fn pin_in_partial_clone_skips_promisor_objects() {
    let sb = Sandbox::isolate();
    let source = base_repo(ObjectFormat::Sha1);
    source.write("dir/c.txt", b"old c\n");
    source.commit("c2");
    let old_c = source.oid("HEAD:dir/c.txt");
    source.write("dir/c.txt", b"new c\n");
    source.commit("c3");
    let clone = source.clone_blobless();
    assert!(!cat_file_exists(&clone, &old_c));
    // A sparse entry whose blob was never fetched, in a directory that changes.
    let cacheinfo = format!("100644,{old_c},dir/c.txt");
    clone.git(&["update-index", "--cacheinfo", &cacheinfo]);
    clone.git(&["update-index", "--skip-worktree", "dir/c.txt"]);
    std::fs::remove_file(clone.path().join("dir/c.txt")).unwrap();
    clone.write("dir/b.txt", b"bravo changed\n");

    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&clone), clone.path()).unwrap();
    assert_eq!(blob_in(&clone, &state, "dir/c.txt"), old_c);
    let changed = blob_in(&clone, &state, "dir/b.txt");

    let name = snap.pin(&info(&clone), &state).unwrap();

    assert_eq!(clone.git(&["rev-parse", &name]), state.head_tree.as_str());
    assert!(cat_file_exists(&clone, &changed));
    assert!(!cat_file_exists(&clone, &old_c), "nothing is ever fetched");
}

#[test]
fn pin_runs_no_reference_transaction_hook() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let marker = sb.home().join("hook-ran");
    let hook = repo.path().join(".git/hooks/reference-transaction");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!("#!/bin/sh\necho \"$1\" >> '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // The hook works for the user's own ref updates.
    repo.git(&["update-ref", "refs/heads/probe", "HEAD"]);
    assert!(marker.exists());
    std::fs::remove_file(&marker).unwrap();

    repo.write("a.txt", b"hooked\n");
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    snap.pin(&info(&repo), &state).unwrap();
    snap.pin_tree(&info(&repo), &repo.oid("HEAD^{tree}"))
        .unwrap();
    snap.delete_unreferenced_refs(&info(&repo), &HashSet::new())
        .unwrap();
    assert!(
        !marker.exists(),
        "the user's reference-transaction hook ran"
    );
}

#[test]
fn pin_honors_core_shared_repository() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.git(&["config", "core.sharedRepository", "group"]);
    let snap = snapshotter(&sb);
    let objects = repo.path().join(".git/objects");
    // Pick content whose blob lands in a fan-out dir the repo does not have yet.
    let (state, blob) = (0..)
        .find_map(|i| {
            repo.write(
                "n/new.txt",
                format!("shared with the group {i}\n").as_bytes(),
            );
            let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
            let blob = blob_in(&repo, &state, "n/new.txt");
            (!objects.join(&blob.as_str()[..2]).exists()).then_some((state, blob))
        })
        .unwrap();
    let fanout = objects.join(&blob.as_str()[..2]);
    let entries_before: HashSet<PathBuf> = std::fs::read_dir(&objects)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();

    snap.pin(&info(&repo), &state).unwrap();

    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(&fanout).unwrap().permissions().mode();
    assert_eq!(mode & 0o070, 0o070, "fan-out dir mode {mode:o}");
    assert!(cat_file_exists(&repo, &blob));
    // No temp files are left in the objects dir.
    let new: Vec<PathBuf> = std::fs::read_dir(&objects)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| !entries_before.contains(p))
        .filter(|p| !p.is_dir())
        .collect();
    assert!(new.is_empty(), "{new:?}");
}

#[test]
fn pin_copies_big_file_streamed_into_a_scratch_pack() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    // `git add` streams blobs above core.bigFileThreshold straight into a pack.
    repo.git(&["config", "core.bigFileThreshold", "1k"]);
    let big: Vec<u8> = (0..20_000u32)
        .flat_map(|i| format!("{i}\n").into_bytes())
        .collect();
    repo.write("big.txt", &big);
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let blob = blob_in(&repo, &state, "big.txt");
    let name = snap.pin(&info(&repo), &state).unwrap();
    std::fs::remove_dir_all(sb.cache_dir().join("polygloss")).unwrap();
    repo.git(&["gc", "-q", "--prune=now"]);
    assert!(cat_file_exists(&repo, &blob));
    assert_eq!(repo.git(&["rev-parse", &name]), state.head_tree.as_str());
}

#[test]
fn pin_is_idempotent() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.write("a.txt", b"twice\n");
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
    let first = snap.pin(&info(&repo), &state).unwrap();
    let count = repo.git(&["count-objects", "-v"]);
    let refs = repo.git(&["for-each-ref"]);
    let second = snap.pin(&info(&repo), &state).unwrap();
    assert_eq!(first, second);
    assert_eq!(repo.git(&["count-objects", "-v"]), count);
    assert_eq!(repo.git(&["for-each-ref"]), refs);
}

#[test]
fn pin_from_linked_worktree_lands_in_common_objects() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let wt = repo.add_worktree("feature");
    std::fs::write(wt.join("wt-only.txt"), b"linked worktree file\n").unwrap();
    let wt_repo = discover(&wt).unwrap();
    let snap = snapshotter(&sb);
    let state = snap.snapshot(&wt_repo, &wt).unwrap();
    let name = snap.pin(&wt_repo, &state).unwrap();
    // Visible from the main worktree: refs and objects are shared.
    assert_eq!(repo.git(&["rev-parse", &name]), state.head_tree.as_str());
    assert!(
        repo.git(&["ls-tree", "--name-only", &name])
            .contains("wt-only.txt")
    );
}

#[test]
fn pin_tree_refs_a_fixed_base() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let base = repo.oid("HEAD^{tree}");
    let snap = snapshotter(&sb);
    let name = snap.pin_tree(&info(&repo), &base).unwrap();
    assert_eq!(name, snapshot_ref(&base));
    assert_eq!(repo.git(&["rev-parse", &name]), base.as_str());
    assert_eq!(snap.pin_tree(&info(&repo), &base).unwrap(), name);
    // A tree the repo does not have is an error, never a dangling ref.
    let bogus = Oid::parse(&"ab".repeat(20), ObjectFormat::Sha1).unwrap();
    assert!(snap.pin_tree(&info(&repo), &bogus).is_err());
    assert!(
        repo.git(&["for-each-ref", &snapshot_ref(&bogus)])
            .is_empty()
    );
}

#[test]
fn prune_scratch_keeps_last_two() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let snap = snapshotter(&sb);
    let mut states = Vec::new();
    let mut blobs = Vec::new();
    for i in 1..=3 {
        repo.write("a.txt", format!("state {i}\n").as_bytes());
        repo.write("shared.txt", b"in every state\n");
        let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
        blobs.push(blob_in(&repo, &state, "a.txt"));
        states.push(state);
    }
    let shared = blob_in(&repo, &states[0], "shared.txt");
    let reader = BlobReader::open(&info(&repo))
        .unwrap()
        .with_scratch(&states[0].scratch_objects)
        .unwrap();
    for blob in &blobs {
        assert!(reader.exists(blob));
    }

    snap.prune_scratch(&info(&repo), repo.path(), 2).unwrap();

    let reader = BlobReader::open(&info(&repo))
        .unwrap()
        .with_scratch(&states[0].scratch_objects)
        .unwrap();
    assert!(matches!(
        reader.read(&blobs[0]),
        Err(ObjectError::Missing(_))
    ));
    assert!(!reader.exists(&states[0].head_tree));
    assert_eq!(&*reader.read(&blobs[1]).unwrap(), b"state 2\n");
    assert_eq!(&*reader.read(&blobs[2]).unwrap(), b"state 3\n");
    assert_eq!(&*reader.read(&shared).unwrap(), b"in every state\n");
    // The kept states still diff.
    let git = snap.diff_env(&states[2], Git::new(repo.path()));
    let changes = list_changes(
        &git,
        ObjectFormat::Sha1,
        &states[1].head_tree,
        &states[2].head_tree,
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    // The next snapshot still works after a prune.
    repo.write("a.txt", b"state 4\n");
    snap.snapshot(&info(&repo), repo.path()).unwrap();
}

#[test]
fn prune_scratch_accepts_a_subdirectory() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let snap = snapshotter(&sb);
    let mut blobs = Vec::new();
    for i in 1..=3 {
        repo.write("a.txt", format!("sub {i}\n").as_bytes());
        let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
        blobs.push((blob_in(&repo, &state, "a.txt"), state));
    }
    let sub = repo.path().join("dir");
    snap.prune_scratch(&discover(&sub).unwrap(), &sub, 1)
        .unwrap();
    let reader = BlobReader::open(&info(&repo))
        .unwrap()
        .with_scratch(&blobs[0].1.scratch_objects)
        .unwrap();
    assert!(!reader.exists(&blobs[0].0));
    assert!(!reader.exists(&blobs[1].0));
    assert!(reader.exists(&blobs[2].0));
}

#[test]
fn prune_scratch_keeps_other_worktrees_states_and_packs_in_use() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    repo.git(&["config", "core.bigFileThreshold", "1k"]);
    let wt = repo.add_worktree("feature");
    let wt_repo = discover(&wt).unwrap();
    let snap = snapshotter(&sb);
    let big: Vec<u8> = (0..20_000u32)
        .flat_map(|i| format!("w{i}\n").into_bytes())
        .collect();
    std::fs::write(wt.join("big.txt"), &big).unwrap();
    let wt_state = snap.snapshot(&wt_repo, &wt).unwrap();
    let big_blob = blob_in(&repo, &wt_state, "big.txt");

    let mut main_blobs = Vec::new();
    for i in 1..=3 {
        repo.write("a.txt", format!("main {i}\n").as_bytes());
        let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
        main_blobs.push(blob_in(&repo, &state, "a.txt"));
    }
    snap.prune_scratch(&info(&repo), repo.path(), 1).unwrap();

    let reader = BlobReader::open(&info(&repo))
        .unwrap()
        .with_scratch(&wt_state.scratch_objects)
        .unwrap();
    assert!(!reader.exists(&main_blobs[0]));
    assert!(!reader.exists(&main_blobs[1]));
    assert!(reader.exists(&main_blobs[2]));
    assert_eq!(&*reader.read(&big_blob).unwrap(), &big[..]);

    // Pruning the linked worktree to zero states drops its packed big blob too.
    snap.prune_scratch(&wt_repo, &wt, 0).unwrap();
    let reader = BlobReader::open(&info(&repo))
        .unwrap()
        .with_scratch(&wt_state.scratch_objects)
        .unwrap();
    assert!(!reader.exists(&big_blob));
    assert!(reader.exists(&main_blobs[2]));
}

#[test]
fn delete_unreferenced_refs_only_touches_polygloss_namespace() {
    let sb = Sandbox::isolate();
    let repo = base_repo(ObjectFormat::Sha1);
    let snap = snapshotter(&sb);
    let mut names = Vec::new();
    for i in 1..=3 {
        repo.write("a.txt", format!("ref {i}\n").as_bytes());
        let state = snap.snapshot(&info(&repo), repo.path()).unwrap();
        names.push(snap.pin(&info(&repo), &state).unwrap());
    }
    repo.branch("keep-me");
    repo.git(&["update-ref", "refs/other/thing", "HEAD"]);
    repo.git(&["update-ref", "refs/polygloss-lookalike/x", "HEAD"]);
    let referenced: HashSet<String> = [names[1].clone()].into_iter().collect();

    let mut deleted = snap
        .delete_unreferenced_refs(&info(&repo), &referenced)
        .unwrap();
    deleted.sort();
    let mut expected = vec![names[0].clone(), names[2].clone()];
    expected.sort();
    assert_eq!(deleted, expected);

    let refs = repo.git(&["for-each-ref", "--format=%(refname)"]);
    let refs: Vec<&str> = refs.lines().collect();
    assert_eq!(
        refs,
        [
            "refs/heads/keep-me",
            "refs/heads/main",
            "refs/other/thing",
            "refs/polygloss-lookalike/x",
            names[1].as_str()
        ]
    );
    // Nothing left to delete the second time.
    assert!(
        snap.delete_unreferenced_refs(&info(&repo), &referenced)
            .unwrap()
            .is_empty()
    );
}

/// Spike S3 (plan T1.5): a 50k-file worktree with 1k modified and 200 untracked
/// files. Run with `scripts/cargo.sh nextest run -p polygloss-core --run-ignored only
/// --no-capture snapshot_cost_large_dirty_worktree`.
#[test]
#[ignore = "spike S3: slow; run explicitly"]
fn snapshot_cost_large_dirty_worktree() {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let setup = Instant::now();
    for d in 0..500 {
        for f in 0..100 {
            repo.write(
                &format!("d{d:03}/f{f:03}.txt"),
                format!("dir {d} file {f}\nline two\nline three\n").as_bytes(),
            );
        }
    }
    repo.commit("50k files");
    for i in 0..1_000 {
        let (d, f) = (i % 500, (i * 7) % 100);
        repo.write(
            &format!("d{d:03}/f{f:03}.txt"),
            format!("modified {i}\n").as_bytes(),
        );
    }
    for i in 0..200 {
        repo.write(
            &format!("untracked/u{i:03}.txt"),
            format!("u {i}\n").as_bytes(),
        );
    }
    eprintln!("S3 setup: {:?}", setup.elapsed());

    let snap = snapshotter(&sb);
    let repo_info = info(&repo);
    let mut times = Vec::new();
    let mut tree = None;
    for _ in 0..5 {
        let t = Instant::now();
        let state = snap.snapshot(&repo_info, repo.path()).unwrap();
        times.push(t.elapsed());
        if let Some(prev) = &tree {
            assert_eq!(prev, &state.head_tree);
        }
        tree = Some(state.head_tree);
    }
    let mut sorted = times.clone();
    sorted.sort();
    eprintln!(
        "S3 snapshot x5: {times:?}; p50 {:?}, max {:?}",
        sorted[2], sorted[4]
    );
    let state = snap.snapshot(&repo_info, repo.path()).unwrap();
    let t = Instant::now();
    snap.pin(&repo_info, &state).unwrap();
    eprintln!("S3 pin: {:?}", t.elapsed());
    let t = Instant::now();
    snap.prune_scratch(&repo_info, repo.path(), 2).unwrap();
    eprintln!("S3 prune_scratch: {:?}", t.elapsed());
}
