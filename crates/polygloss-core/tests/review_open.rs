//! Review open orchestration: repos, reviews, diffs, iterations, pinning and prune
//! (T1.12, design §3–§5, §7.2–§7.4, OQ-2, OQ-24, OQ-34).
//!
//! Every test runs under `Sandbox::isolate()` (temp `HOME`, data dir and git
//! config), so the store and the scratch stores live in the sandbox. Only nextest
//! (one process per test) is supported.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use polygloss_core::git::{CompareMode, ReviewKind, Since, Source, snapshot_ref};
use polygloss_core::review::{Core, CoreError, OpenRequest, PinnedBy};
use polygloss_core::store::events::{Actor, EventFilter, events_since};
use polygloss_core::testing::{self, FixtureRepo, Sandbox};
use polygloss_core::{ObjectFormat, Oid, diff_id};
use polygloss_diff::{FileKind, FileStatus};

const DAY_MS: i64 = 86_400_000;

fn core() -> Core {
    Core::open_default().expect("open core in the sandbox")
}

fn agent() -> Actor {
    Actor {
        kind: polygloss_core::store::events::ActorKind::Agent,
        name: Some("claude-code".into()),
        session_id: Some("s-1".into()),
    }
}

fn req(worktree: &Path, source: Source) -> OpenRequest {
    OpenRequest {
        worktree: worktree.to_path_buf(),
        source,
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn compare(base: &str, head: &str) -> Source {
    Source::Compare {
        base: base.into(),
        head: head.into(),
        mode: CompareMode::ThreeDot,
    }
}

fn commit(rev: &str) -> Source {
    Source::Commit { rev: rev.into() }
}

fn live(since: Since) -> Source {
    Source::Live { since }
}

/// `main` with `a.txt` (c1), then branch `feature` with a change and a new file.
fn feature_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\ntwo\nthree\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", b"one\n2\nthree\n");
    repo.write("b.txt", b"bee\n");
    repo.commit("f1");
    repo.checkout("main");
    repo
}

fn count(core: &Core, sql: &str) -> i64 {
    core.store
        .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
        .unwrap()
}

fn text(core: &Core, sql: &str) -> Option<String> {
    core.store
        .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, Option<String>>(0))?))
        .unwrap()
}

fn exec(core: &Core, sql: &str) {
    core.store
        .write(|tx| {
            tx.execute_batch(sql)?;
            Ok(())
        })
        .unwrap();
}

/// `(kind, review_id, payload)` of every event, oldest first.
fn events(core: &Core) -> Vec<(String, Option<String>, serde_json::Value)> {
    core.store
        .read(|c| events_since(c, 0, &EventFilter::default(), 10_000))
        .unwrap()
        .into_iter()
        .map(|e| (e.kind.as_str().to_owned(), e.review_id, e.payload))
        .collect()
}

fn polygloss_refs(repo_dir: &Path) -> BTreeSet<String> {
    let out = std::process::Command::new("git")
        .current_dir(repo_dir)
        .args(["for-each-ref", "--format=%(refname)", "refs/polygloss/"])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `git clone -q <src> <dst>` with the fixture's hermetic git.
fn clone_to(src: &FixtureRepo, dst: &Path) {
    src.git(&[
        "clone",
        "-q",
        src.path().to_str().unwrap(),
        dst.to_str().unwrap(),
    ]);
}

fn tree_of(repo: &FixtureRepo, rev: &str) -> Oid {
    repo.oid(&format!("{rev}^{{tree}}"))
}

#[test]
fn open_commit_creates_review_and_iteration_1() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let head = repo.oid("feature");

    let opened = core.open(&req(repo.path(), commit("feature"))).unwrap();

    assert_eq!(opened.kind, ReviewKind::Commit);
    assert_eq!(opened.review_key, format!("commit:{head}"));
    assert_eq!(opened.base.tree, tree_of(&repo, "main"));
    assert_eq!(opened.head_tree, tree_of(&repo, "feature"));
    assert_eq!(opened.head_commit.as_ref(), Some(&head));
    assert_eq!(
        opened.diff_id,
        diff_id(ObjectFormat::Sha1, &opened.base.tree, &opened.head_tree)
    );
    let it = opened
        .iteration
        .clone()
        .expect("commit opens record iteration 1");
    assert_eq!(it.seq, 1);
    assert_eq!(it.diff_id, opened.diff_id);
    assert_eq!(it.snapshot_ref, None);
    assert!(opened.live.is_none());
    assert_eq!(opened.files.len(), 2);
    assert_eq!(opened.repo.common_dir, repo.path().join(".git"));

    assert_eq!(count(&core, "SELECT count(*) FROM repos"), 1);
    assert_eq!(
        text(&core, "SELECT display_name FROM repos").as_deref(),
        Some("repo")
    );
    assert_eq!(
        text(&core, "SELECT kind FROM reviews").as_deref(),
        Some("commit")
    );
    assert_eq!(
        text(&core, "SELECT pinned_by FROM iterations").as_deref(),
        Some("open")
    );
    assert_eq!(
        text(&core, "SELECT head_commit FROM iterations").as_deref(),
        Some(head.as_str())
    );
    assert_eq!(count(&core, "SELECT files_count FROM diffs"), 2);
    assert_eq!(count(&core, "SELECT count(*) FROM file_changes"), 2);

    let ev = events(&core);
    let kinds: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(kinds, ["review.created", "iteration.created"]);
    assert_eq!(ev[0].1.as_deref(), Some(opened.review_id.as_str()));
    assert_eq!(ev[0].2["key"], opened.review_key.as_str());
    assert_eq!(ev[0].2["kind"], "commit");
    assert_eq!(ev[1].2["seq"], 1);
    assert_eq!(ev[1].2["diff_id"], opened.diff_id.as_str());
    assert_eq!(ev[1].2["pinned_by"], "open");
    assert_eq!(core.iterations(&opened.review_id).unwrap(), vec![it]);
}

#[test]
fn open_same_compare_twice_reuses_iteration() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();

    let a = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let b = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    assert_eq!(a.review_id, b.review_id);
    assert_eq!(a.review_key, "compare:refs/heads/main...refs/heads/feature");
    assert_eq!(a.iteration, b.iteration);
    assert_eq!(count(&core, "SELECT count(*) FROM reviews"), 1);
    assert_eq!(count(&core, "SELECT count(*) FROM iterations"), 1);
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 1);
    assert_eq!(
        text(
            &core,
            "SELECT base_spec || ' ' || head_spec || ' ' || compare_mode FROM reviews"
        )
        .as_deref(),
        Some("refs/heads/main refs/heads/feature three-dot")
    );
    let kinds: Vec<String> = events(&core).into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, ["review.created", "iteration.created"]);
}

#[test]
fn open_compare_after_ref_moves_creates_iteration_2() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let first = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    repo.checkout("feature");
    repo.write("c.txt", b"sea\n");
    repo.commit("f2");
    let mut refresh = req(repo.path(), compare("main", "feature"));
    refresh.pin = Some(PinnedBy::Refresh);
    let second = core.open(&refresh).unwrap();

    assert_eq!(first.review_id, second.review_id);
    assert_ne!(first.diff_id, second.diff_id);
    let its = core.iterations(&second.review_id).unwrap();
    assert_eq!(its.len(), 2);
    assert_eq!((its[0].seq, its[1].seq), (1, 2));
    assert_eq!(its[1].diff_id, second.diff_id);
    assert_eq!(second.iteration.as_ref(), Some(&its[1]));
    assert_eq!(
        text(&core, "SELECT pinned_by FROM iterations WHERE seq = 2").as_deref(),
        Some("refresh")
    );
    assert_eq!(second.files.len(), 3);
}

#[test]
fn open_live_unpinned_creates_review_without_iteration() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    repo.write("untracked.txt", b"new\n");
    let core = core();

    let opened = core.open(&req(repo.path(), live(Since::Head))).unwrap();

    assert_eq!(opened.kind, ReviewKind::Live);
    assert_eq!(
        opened.review_key,
        format!("worktree:{}@main#since=HEAD", repo.path().display())
    );
    assert!(opened.iteration.is_none());
    let state = opened.live.as_ref().expect("live opens carry their state");
    assert_eq!(opened.head_tree, state.head_tree);
    assert_eq!(opened.head_commit, None);
    let paths: Vec<&str> = opened.files.iter().map(|f| f.display_path()).collect();
    assert_eq!(paths, ["untracked.txt"]);

    assert_eq!(count(&core, "SELECT count(*) FROM reviews"), 1);
    assert_eq!(
        text(&core, "SELECT since FROM reviews").as_deref(),
        Some("HEAD")
    );
    assert_eq!(
        text(&core, "SELECT worktree_path FROM reviews").as_deref(),
        Some(repo.path().to_str().unwrap())
    );
    assert_eq!(count(&core, "SELECT count(*) FROM iterations"), 0);
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 0);
    assert_eq!(count(&core, "SELECT count(*) FROM file_changes"), 0);
    assert!(polygloss_refs(repo.path()).is_empty());
    assert!(core.files_for_diff(&opened.diff_id).unwrap().is_none());
    let kinds: Vec<String> = events(&core).into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, ["review.created"]);
}

#[test]
fn pin_live_creates_iteration_with_snapshot_ref() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    repo.write("untracked.txt", b"new\n");
    let core = core();
    let opened = core.open(&req(repo.path(), live(Since::Head))).unwrap();
    let state = opened.live.clone().unwrap();

    let it = core
        .pin_live(&opened.review_id, &state, PinnedBy::Manual, &Actor::human())
        .unwrap();

    let expected_ref = snapshot_ref(&state.head_tree);
    assert_eq!(it.seq, 1);
    assert_eq!(it.diff_id, opened.diff_id);
    assert_eq!(it.snapshot_ref.as_deref(), Some(expected_ref.as_str()));
    assert!(polygloss_refs(repo.path()).contains(&expected_ref));
    assert_eq!(
        text(&core, "SELECT pinned_by FROM iterations").as_deref(),
        Some("manual")
    );
    let files = core.files_for_diff(&it.diff_id).unwrap().unwrap();
    assert_eq!(*files, *opened.files);

    // Pinning the same state again reuses the iteration.
    let again = core
        .pin_live(
            &opened.review_id,
            &state,
            PinnedBy::Comment,
            &Actor::human(),
        )
        .unwrap();
    assert_eq!(again, it);
    assert_eq!(count(&core, "SELECT count(*) FROM iterations"), 1);

    let ev = events(&core);
    assert_eq!(ev.last().unwrap().0, "iteration.created");
    assert_eq!(ev.last().unwrap().2["pinned_by"], "manual");
    assert_eq!(ev.iter().filter(|e| e.0 == "iteration.created").count(), 1);

    // Reopening the unchanged worktree reports the pinned iteration.
    let reopened = core.open(&req(repo.path(), live(Since::Head))).unwrap();
    assert_eq!(reopened.iteration, Some(it));
}

#[test]
fn open_live_with_pin_records_iteration_and_since_commit_pins_its_base() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let c1 = repo.oid("main");
    repo.write("a.txt", b"x\n");
    repo.commit("c2");
    repo.write("a.txt", b"y\n");
    let core = core();

    let mut r = req(repo.path(), live(Since::Commit(c1.to_string())));
    r.pin = Some(PinnedBy::Agent);
    r.actor = agent();
    let opened = core.open(&r).unwrap();

    let it = opened
        .iteration
        .clone()
        .expect("pin set: iteration recorded");
    assert_eq!(opened.base.tree, tree_of(&repo, &c1.to_string()));
    let refs = polygloss_refs(repo.path());
    assert!(refs.contains(&snapshot_ref(&opened.head_tree)));
    assert!(refs.contains(&snapshot_ref(&opened.base.tree)), "{refs:?}");
    assert_eq!(it.snapshot_ref, Some(snapshot_ref(&opened.head_tree)));
    assert_eq!(
        text(&core, "SELECT pinned_by FROM iterations").as_deref(),
        Some("agent")
    );
    assert_eq!(
        text(
            &core,
            "SELECT actor_name FROM events WHERE kind = 'iteration.created'"
        )
        .as_deref(),
        Some("claude-code")
    );
}

#[test]
fn pin_live_rejects_a_state_from_another_worktree_or_a_non_live_review() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let wt = repo.add_worktree("wt");
    let core = core();
    let main = core.open(&req(repo.path(), live(Since::Head))).unwrap();
    let other = core.open(&req(&wt, live(Since::Head))).unwrap();
    let cmp = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    let err = core
        .pin_live(
            &main.review_id,
            other.live.as_ref().unwrap(),
            PinnedBy::Manual,
            &Actor::human(),
        )
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict(_)), "{err:?}");
    let err = core
        .pin_live(
            &cmp.review_id,
            main.live.as_ref().unwrap(),
            PinnedBy::Manual,
            &Actor::human(),
        )
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict(_)), "{err:?}");
    let err = core
        .pin_live(
            "nope",
            main.live.as_ref().unwrap(),
            PinnedBy::Manual,
            &Actor::human(),
        )
        .unwrap_err();
    assert!(matches!(err, CoreError::NotFound { .. }), "{err:?}");
    assert!(polygloss_refs(repo.path()).is_empty());
}

#[test]
fn linked_worktrees_share_repo_row() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let wt = repo.add_worktree("wt");
    std::fs::create_dir_all(wt.join("sub")).unwrap();
    let core = core();

    let a = core.open(&req(repo.path(), live(Since::Head))).unwrap();
    let b = core.open(&req(&wt.join("sub"), live(Since::Head))).unwrap();

    assert_eq!(a.repo_id, b.repo_id);
    assert_eq!(a.repo.common_dir, b.repo.common_dir);
    assert_ne!(a.review_id, b.review_id);
    assert_eq!(
        b.review_key,
        format!("worktree:{}@wt#since=HEAD", wt.display())
    );
    assert_eq!(count(&core, "SELECT count(*) FROM repos"), 1);
    assert_eq!(count(&core, "SELECT count(*) FROM reviews"), 2);
    assert_eq!(
        text(&core, "SELECT display_name FROM repos").as_deref(),
        Some("repo")
    );
}

#[test]
fn file_changes_cached_by_diff_id() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();

    let first = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let diff_trees = testing::git_spawns("diff-tree");
    let check_attrs = testing::git_spawns("check-attr");
    assert!(diff_trees >= 1);

    // Same trees through another review (commit instead of compare).
    let second = core.open(&req(repo.path(), commit("feature"))).unwrap();

    assert_eq!(first.diff_id, second.diff_id);
    assert_eq!(testing::git_spawns("diff-tree"), diff_trees);
    assert_eq!(testing::git_spawns("check-attr"), check_attrs);
    assert_eq!(*first.files, *second.files);
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 1);
    assert_eq!(count(&core, "SELECT count(*) FROM reviews"), 2);
}

#[test]
fn file_changes_roundtrip_every_field_through_the_table() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("old-name.txt", b"line 1\nline 2\nline 3\nline 4\nline 5\n");
    repo.write("script.sh", b"echo hi\n");
    repo.write("gone.txt", b"bye\n");
    repo.write(".gitattributes", b"*.bin binary\n");
    repo.commit("c1");
    std::fs::rename(
        repo.path().join("old-name.txt"),
        repo.path().join("new name.txt"),
    )
    .unwrap();
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
    std::fs::set_permissions(
        repo.path().join("script.sh"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    repo.write("data.bin", b"abc\n");
    repo.write("Cargo.lock", b"# lock\n");
    std::os::unix::fs::symlink("script.sh", repo.path().join("link")).unwrap();
    // APFS rejects non-UTF-8 file names, so that entry goes straight into the index.
    repo.write("latin.tmp", b"latin-1 name\n");
    let blob = repo.git(&["hash-object", "-w", "latin.tmp"]);
    std::fs::remove_file(repo.path().join("latin.tmp")).unwrap();
    repo.git(&["add", "-A"]);
    let cacheinfo = [b"100644,", blob.as_bytes(), b",caf\xe9.txt"].concat();
    let status = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["update-index", "--add", "--cacheinfo"])
        .arg(OsStr::from_bytes(&cacheinfo))
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["commit", "-q", "-m", "c2"]);
    let core = core();

    let opened = core.open(&req(repo.path(), commit("HEAD"))).unwrap();
    let cached = core.files_for_diff(&opened.diff_id).unwrap().unwrap();

    assert_eq!(*cached, *opened.files);
    let by_path = |p: &str| {
        cached
            .iter()
            .find(|f| f.display_path() == p)
            .unwrap_or_else(|| panic!("{p} missing in {cached:?}"))
    };
    let renamed = by_path("new name.txt");
    assert_eq!(renamed.status, FileStatus::Renamed);
    assert!(renamed.similarity.is_some());
    assert_eq!(by_path("gone.txt").status, FileStatus::Deleted);
    assert!(by_path("gone.txt").new_blob.is_zero());
    assert_eq!(by_path("data.bin").kind, FileKind::Binary);
    assert!(by_path("Cargo.lock").generated);
    assert_eq!(by_path("link").kind, FileKind::Symlink);
    let script = by_path("script.sh");
    assert_eq!(
        (script.old_mode.unwrap().0, script.new_mode.unwrap().0),
        (0o100644, 0o100755)
    );
    let escaped = cached
        .iter()
        .find(|f| f.new_path.as_ref().is_some_and(|p| p.escaped))
        .expect("non-UTF-8 path keeps its escaped flag");
    assert_eq!(
        escaped.new_path.as_ref().unwrap().to_bytes(),
        b"caf\xe9.txt"
    );
    let idx: Vec<u32> = cached.iter().map(|f| f.idx).collect();
    assert_eq!(idx, (0..cached.len() as u32).collect::<Vec<_>>());
}

#[test]
fn amend_keeps_diff_id() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let before = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    repo.checkout("feature");
    repo.git(&["commit", "-q", "--amend", "-m", "reworded"]);
    repo.checkout("main");
    let after = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let commit_review = core.open(&req(repo.path(), commit("feature"))).unwrap();

    assert_ne!(before.head_commit, after.head_commit);
    assert_eq!(before.diff_id, after.diff_id);
    assert_eq!(before.iteration, after.iteration);
    assert_eq!(commit_review.diff_id, before.diff_id);
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 1);
    assert_eq!(
        count(
            &core,
            &format!(
                "SELECT count(*) FROM iterations WHERE review_id = '{}'",
                before.review_id
            )
        ),
        1
    );
}

#[test]
fn diff_id_same_across_clones() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let dir = tempfile::tempdir().unwrap();
    let clone = std::fs::canonicalize(dir.path()).unwrap().join("clone");
    clone_to(&repo, &clone);
    let core = core();

    let a = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let b = core
        .open(&req(&clone, compare("origin/main", "origin/feature")))
        .unwrap();

    assert_eq!(a.diff_id, b.diff_id);
    assert_ne!(a.repo_id, b.repo_id);
    assert_ne!(a.review_id, b.review_id);
    assert_eq!(
        b.review_key,
        "compare:refs/remotes/origin/main...refs/remotes/origin/feature"
    );
    assert_eq!(count(&core, "SELECT count(*) FROM repos"), 2);
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 1);
    assert_eq!(count(&core, "SELECT count(*) FROM iterations"), 2);
}

#[test]
fn find_repo_for_diff_by_prefix_and_ambiguity_error() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let unrelated = FixtureRepo::init(ObjectFormat::Sha1);
    unrelated.write("z.txt", b"z\n");
    unrelated.commit("z");
    let core = core();
    let opened = core.open(&req(repo.path(), commit("feature"))).unwrap();
    let id = opened.diff_id.as_str();

    // Unique prefix, found through the iteration's repo even from an unrelated cwd.
    let (found, full) = core
        .find_repo_for_diff(&id[..8], Some(unrelated.path()))
        .unwrap();
    assert_eq!(full, opened.diff_id);
    assert_eq!(found.common_dir, opened.repo.common_dir);
    assert_eq!(found.toplevel.as_deref(), Some(repo.path()));
    // Full id with cwd inside the repo.
    let (found, _) = core.find_repo_for_diff(id, Some(repo.path())).unwrap();
    assert_eq!(found.common_dir, opened.repo.common_dir);

    // Two diffs sharing an 8-char prefix.
    let fake = |c: char| format!("abcdef01{}", c.to_string().repeat(56));
    for c in ['1', '2'] {
        exec(
            &core,
            &format!(
                "INSERT INTO diffs (id, object_format, base_tree, head_tree, created_at) \
                 VALUES ('{}', 'sha1', '{}', '{}', 0)",
                fake(c),
                "1".repeat(40),
                "2".repeat(40)
            ),
        );
    }
    let err = core.find_repo_for_diff("abcdef01", None).unwrap_err();
    match err {
        CoreError::Ambiguous { prefix, matches } => {
            assert_eq!(prefix, "abcdef01");
            assert_eq!(matches, vec![fake('1'), fake('2')]);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    // Unique but no known repo has its trees.
    let err = core.find_repo_for_diff("abcdef011", None).unwrap_err();
    assert!(matches!(err, CoreError::RepoNotFound(_)), "{err:?}");
    assert_eq!(err.code(), "repo_not_found");
    // Unknown id and too-short prefix.
    let err = core.find_repo_for_diff("ffffffff", None).unwrap_err();
    assert!(matches!(err, CoreError::NotFound { .. }), "{err:?}");
    assert_eq!(err.code(), "not_found");
    assert!(core.find_repo_for_diff("abc", None).is_err());
}

#[test]
fn label_is_display_only() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let mut labeled = req(repo.path(), compare("main", "feature"));
    labeled.label = Some("PR #123".into());

    let a = core.open(&labeled).unwrap();
    let b = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    assert_eq!(a.review_key, "compare:refs/heads/main...refs/heads/feature");
    assert_eq!(a.review_id, b.review_id);
    assert_eq!(a.diff_id, b.diff_id);
    assert_eq!(
        text(&core, "SELECT label FROM reviews").as_deref(),
        Some("PR #123")
    );
    labeled.label = Some("PR #124".into());
    let c = core.open(&labeled).unwrap();
    assert_eq!(c.review_id, a.review_id);
    assert_eq!(
        text(&core, "SELECT label FROM reviews").as_deref(),
        Some("PR #124")
    );
    assert_eq!(count(&core, "SELECT count(*) FROM reviews"), 1);
}

#[test]
fn archive_review_records_event_and_reopening_unarchives() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    core.archive_review(&opened.review_id, &Actor::human())
        .unwrap();
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM reviews WHERE archived_at IS NOT NULL"
        ),
        1
    );
    let ev = events(&core);
    let last = ev.last().unwrap();
    assert_eq!(last.0, "review.archived");
    assert_eq!(last.2["key"], opened.review_key.as_str());
    assert_eq!(last.2["kind"], "compare");
    // Archiving twice is a no-op.
    core.archive_review(&opened.review_id, &Actor::human())
        .unwrap();
    assert_eq!(events(&core).len(), ev.len());
    assert!(matches!(
        core.archive_review("missing", &Actor::human()),
        Err(CoreError::NotFound { .. })
    ));

    core.open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM reviews WHERE archived_at IS NULL"
        ),
        1
    );
}

/// Inserts a review-level thread with one comment on `review_id`'s first diff.
fn add_thread(core: &Core, review_id: &str, kind: &str, author: &str, published: bool) -> String {
    let thread = polygloss_core::new_uuid();
    let comment = polygloss_core::new_uuid();
    let published_at = if published { "1" } else { "NULL" };
    exec(
        core,
        &format!(
            "INSERT INTO threads (id, review_id, origin_diff_id, subject, kind, created_by_kind, \
               created_by_name, created_at, updated_at) \
             SELECT '{thread}', '{review_id}', diff_id, 'review', '{kind}', '{author}', 'x', 1, 1 \
             FROM iterations WHERE review_id = '{review_id}' ORDER BY seq LIMIT 1; \
             INSERT INTO comments (id, thread_id, author_kind, author_name, body_md, published_at, created_at) \
             VALUES ('{comment}', '{thread}', '{author}', 'x', 'body', {published_at}, 1);"
        ),
    );
    thread
}

fn add_reply(core: &Core, thread: &str, author: &str, published: bool) {
    let published_at = if published { "2" } else { "NULL" };
    exec(
        core,
        &format!(
            "INSERT INTO comments (id, thread_id, author_kind, author_name, body_md, published_at, created_at) \
             VALUES ('{}', '{thread}', '{author}', 'x', 'reply', {published_at}, 2);",
            polygloss_core::new_uuid()
        ),
    );
}

fn review_exists(core: &Core, id: &str) -> bool {
    count(
        core,
        &format!("SELECT count(*) FROM reviews WHERE id = '{id}'"),
    ) == 1
}

#[test]
fn prune_cascades_and_deletes_unreferenced_snapshot_refs() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let c1 = repo.oid("main");
    repo.write("a.txt", b"c2\n");
    repo.commit("c2");
    repo.write("a.txt", b"dirty 1\n");
    let core = core();

    // A: live since=<c1>, pinned twice (S1, then S2). B: live since=HEAD, pinned at S1.
    let a = core
        .open(&req(repo.path(), live(Since::Commit(c1.to_string()))))
        .unwrap();
    let s1 = a.live.clone().unwrap();
    let a1 = core
        .pin_live(&a.review_id, &s1, PinnedBy::Manual, &Actor::human())
        .unwrap();
    let b = core.open(&req(repo.path(), live(Since::Head))).unwrap();
    assert_eq!(b.head_tree, s1.head_tree);
    let b1 = core
        .pin_live(
            &b.review_id,
            b.live.as_ref().unwrap(),
            PinnedBy::Manual,
            &Actor::human(),
        )
        .unwrap();
    repo.write("a.txt", b"dirty 2\n");
    let a_again = core
        .open(&req(repo.path(), live(Since::Commit(c1.to_string()))))
        .unwrap();
    let s2 = a_again.live.clone().unwrap();
    let a2 = core
        .pin_live(&a.review_id, &s2, PinnedBy::Manual, &Actor::human())
        .unwrap();
    let base_ref = snapshot_ref(&tree_of(&repo, &c1.to_string()));
    let before = polygloss_refs(repo.path());
    assert!(before.contains(&base_ref));
    assert!(before.contains(&snapshot_ref(&s1.head_tree)));
    assert!(before.contains(&snapshot_ref(&s2.head_tree)));

    // Rows hanging off A.
    let thread = add_thread(&core, &a.review_id, "comment", "human", false);
    let a_id = &a.review_id;
    exec(
        &core,
        &format!(
            "INSERT INTO review_submissions (id, review_id, iteration_id, verdict, comment_count, submitted_at) \
               VALUES ('sub-1', '{a_id}', {}, 'comment', 0, 1); \
             INSERT INTO review_drafts (review_id, summary_md, updated_at) VALUES ('{a_id}', 'wip', 1); \
             INSERT INTO sessions (id, client_name, first_seen_at, last_seen_at) VALUES ('s-1', 'claude-code', 1, 1); \
             INSERT INTO review_assignments (review_id, session_id, assigned_at, assigned_by) \
               VALUES ('{a_id}', 's-1', 1, 'open_diff'); \
             INSERT INTO view_state (diff_id, state_json, updated_at) VALUES ('{}', '{{\"v\":1}}', 1); \
             INSERT INTO viewed_files (path, old_blob, new_blob, review_id, viewed_at) \
               VALUES ('a.txt', '{}', '{}', '{a_id}', 1);",
            a1.id,
            a2.diff_id,
            "0".repeat(40),
            "1".repeat(40),
        ),
    );

    core.prune_review(&a.review_id).unwrap();

    assert!(!review_exists(&core, &a.review_id));
    assert!(review_exists(&core, &b.review_id));
    for table in [
        "iterations WHERE review_id = '",
        "review_submissions WHERE review_id = '",
        "review_drafts WHERE review_id = '",
        "review_assignments WHERE review_id = '",
        "threads WHERE review_id = '",
    ] {
        assert_eq!(
            count(&core, &format!("SELECT count(*) FROM {table}{a_id}'")),
            0,
            "{table}"
        );
    }
    assert_eq!(
        count(
            &core,
            &format!("SELECT count(*) FROM comments WHERE thread_id = '{thread}'")
        ),
        0
    );
    // A's own diffs are gone (with their file_changes and view_state); B's shared one stays.
    assert_eq!(
        count(
            &core,
            &format!("SELECT count(*) FROM diffs WHERE id = '{}'", a1.diff_id)
        ),
        0
    );
    assert_eq!(
        count(
            &core,
            &format!("SELECT count(*) FROM diffs WHERE id = '{}'", a2.diff_id)
        ),
        0
    );
    assert_eq!(
        count(
            &core,
            &format!(
                "SELECT count(*) FROM file_changes WHERE diff_id = '{}'",
                a2.diff_id
            )
        ),
        0
    );
    assert_eq!(count(&core, "SELECT count(*) FROM view_state"), 0);
    assert_eq!(
        count(
            &core,
            &format!("SELECT count(*) FROM diffs WHERE id = '{}'", b1.diff_id)
        ),
        1
    );
    // Viewed marks are global: kept, detached from the pruned review.
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM viewed_files WHERE review_id IS NULL"
        ),
        1
    );
    // Refs: only S1 (still used by B) survives.
    let after = polygloss_refs(repo.path());
    assert_eq!(
        after,
        BTreeSet::from([snapshot_ref(&s1.head_tree)]),
        "before: {before:?}"
    );
    let ev = events(&core);
    let last = ev.last().unwrap();
    assert_eq!(last.0, "review.archived");
    assert_eq!(last.1.as_deref(), Some(a.review_id.as_str()));
    assert_eq!(last.2["pruned"], true);
    assert!(matches!(
        core.prune_review(&a.review_id),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn prune_waits_for_a_pin_in_flight() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    repo.write("a.txt", b"dirty\n");
    let core = core();
    let victim = core.open(&req(repo.path(), commit("feature"))).unwrap();
    let opened = core.open(&req(repo.path(), live(Since::Head))).unwrap();
    let state = opened.live.clone().unwrap();
    let pause = Duration::from_millis(800);
    testing::pause_after_snapshot_pin(pause);

    let (tx, rx) = mpsc::channel();
    let pinner = {
        let core = core.clone();
        let review = opened.review_id.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let it = core
                .pin_live(&review, &state, PinnedBy::Comment, &Actor::human())
                .unwrap();
            tx.send(started.elapsed()).unwrap();
            it
        })
    };
    // Let the pinner create its ref and reach the pause.
    let pinned_ref = snapshot_ref(&opened.head_tree);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !polygloss_refs(repo.path()).contains(&pinned_ref) {
        assert!(Instant::now() < deadline, "the pin never created its ref");
        std::thread::sleep(Duration::from_millis(10));
    }
    core.prune_review(&victim.review_id).unwrap();
    let it = pinner.join().unwrap();
    let _elapsed = rx.recv().unwrap();

    assert_eq!(it.snapshot_ref.as_deref(), Some(pinned_ref.as_str()));
    assert!(
        polygloss_refs(repo.path()).contains(&pinned_ref),
        "prune deleted the ref of a pin that was about to commit"
    );
    assert!(!review_exists(&core, &victim.review_id));
}

#[test]
fn orphaned_review_after_clone_move_is_kept() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let clone = base.join("clone");
    clone_to(&repo, &clone);
    let core = core();
    let orphan = core
        .open(&req(&clone, compare("origin/main", "origin/feature")))
        .unwrap();
    let kept_repo = core.open(&req(repo.path(), commit("feature"))).unwrap();
    let moved: PathBuf = base.join("moved");
    std::fs::rename(&clone, &moved).unwrap();

    let pruned = core
        .prune_stale(1, polygloss_core::store::events::now_ms() + 30 * DAY_MS)
        .unwrap();

    assert_eq!(pruned, vec![kept_repo.review_id.clone()]);
    assert!(review_exists(&core, &orphan.review_id));
    // Threads survive through diff_id: the moved clone opens the same diff in a new repo row.
    let reopened = core
        .open(&req(&moved, compare("origin/main", "origin/feature")))
        .unwrap();
    assert_eq!(reopened.diff_id, orphan.diff_id);
    assert_ne!(reopened.repo_id, orphan.repo_id);
    // find_repo_for_diff skips the orphan's vanished repo.
    let (found, _) = core
        .find_repo_for_diff(orphan.diff_id.as_str(), None)
        .unwrap();
    assert_eq!(found.common_dir, moved.join(".git"));
    // An explicit prune still removes the orphan (no refs to clean up).
    core.prune_review(&orphan.review_id).unwrap();
    assert!(!review_exists(&core, &orphan.review_id));
}

#[test]
fn prune_stale_deletes_only_old_reviews() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let old = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let recent = core.open(&req(repo.path(), commit("feature"))).unwrap();
    let now = polygloss_core::store::events::now_ms();
    exec(
        &core,
        &format!(
            "UPDATE reviews SET updated_at = {} WHERE id = '{}'",
            now - 31 * DAY_MS,
            old.review_id
        ),
    );
    exec(
        &core,
        &format!(
            "UPDATE reviews SET updated_at = {} WHERE id = '{}'",
            now - 29 * DAY_MS,
            recent.review_id
        ),
    );

    let pruned = core.prune_stale(30, now).unwrap();

    assert_eq!(pruned, vec![old.review_id.clone()]);
    assert!(!review_exists(&core, &old.review_id));
    assert!(review_exists(&core, &recent.review_id));
    // The shared diff stays while the recent review's iteration references it.
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 1);
    assert_eq!(core.prune_stale(30, now).unwrap(), Vec::<String>::new());
}

#[test]
fn prune_stale_skips_orphans_drafts_and_awaiting_you() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    repo.checkout("feature");
    let mut commits = Vec::new();
    for i in 0..5 {
        repo.write("n.txt", format!("{i}\n").as_bytes());
        commits.push(repo.commit(&format!("n{i}")).to_string());
    }
    let dir = tempfile::tempdir().unwrap();
    let clone = std::fs::canonicalize(dir.path()).unwrap().join("clone");
    clone_to(&repo, &clone);
    let core = core();
    let open_commit = |i: usize| core.open(&req(repo.path(), commit(&commits[i]))).unwrap();

    let plain = open_commit(0);
    let with_draft = open_commit(1);
    add_thread(&core, &with_draft.review_id, "comment", "human", false);
    let rereview = open_commit(2);
    exec(
        &core,
        &format!(
            "UPDATE reviews SET status = 'rereview_requested' WHERE id = '{}'",
            rereview.review_id
        ),
    );
    let question = open_commit(3);
    let q = add_thread(&core, &question.review_id, "question", "agent", true);
    add_reply(&core, &q, "human", false); // a draft reply does not answer it...
    let answered = open_commit(4);
    let q2 = add_thread(&core, &answered.review_id, "question", "agent", true);
    add_reply(&core, &q2, "human", true); // ...a published one does.
    let orphan = core.open(&req(&clone, commit("HEAD"))).unwrap();
    std::fs::remove_dir_all(&clone).unwrap();
    exec(&core, "UPDATE reviews SET updated_at = 0");

    let mut pruned = core
        .prune_stale(1, polygloss_core::store::events::now_ms())
        .unwrap();
    pruned.sort();

    let mut expected = vec![plain.review_id.clone(), answered.review_id.clone()];
    expected.sort();
    assert_eq!(pruned, expected);
    for kept in [&with_draft, &rereview, &question, &orphan] {
        assert!(review_exists(&core, &kept.review_id), "{}", kept.review_key);
    }
}

#[test]
fn open_errors_map_to_agent_codes() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let err = core
        .open(&req(repo.path(), commit("no-such-branch")))
        .unwrap_err();
    assert!(matches!(err, CoreError::Resolve(_)), "{err:?}");
    let not_repo = tempfile::tempdir().unwrap();
    let err = core
        .open(&req(not_repo.path(), commit("HEAD")))
        .unwrap_err();
    assert!(matches!(err, CoreError::Git(_)), "{err:?}");
    assert_eq!(err.code(), "repo_not_found");
    assert_eq!(count(&core, "SELECT count(*) FROM reviews"), 0);
    assert!(matches!(
        core.iterations("missing"),
        Err(CoreError::NotFound { .. })
    ));
}
