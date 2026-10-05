//! Viewed, view state, sessions, assignments, waiters and review summaries (T1.14,
//! design §7.2, §9, §11.2, §11.12, §16.3, §16.4, §17, OQ-8, OQ-32).
//!
//! Every test runs under `Sandbox::isolate()`. Threads, comments and submissions
//! (T1.13) are inserted with plain SQL, so these tests only depend on the schema.

use std::collections::BTreeMap;
use std::path::Path;

use polygloss_core::git::{CompareMode, ReviewKind, Since, Source};
use polygloss_core::review::{
    AssignedBy, Core, CoreError, OpenRequest, OpenedDiff, ReviewFilter, ScrollAnchorState,
    SessionInfo, Verdict, ViewState, ViewedState,
};
use polygloss_core::store::events::{Actor, ActorKind, EventFilter, events_since};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::{DiffId, ObjectFormat, Oid};
use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, Side};
use rusqlite::params;

fn core() -> Core {
    Core::open_default().expect("open core in the sandbox")
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

/// `main` with `a.txt` (c1), then branch `feature` changing it and adding `b.txt`.
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

/// Commits `files` on `feature` (and returns to `main`).
fn commit_on_feature(repo: &FixtureRepo, files: &[(&str, &[u8])], msg: &str) {
    repo.checkout("feature");
    for (path, bytes) in files {
        repo.write(path, bytes);
    }
    repo.commit(msg);
    repo.checkout("main");
}

fn file<'a>(opened: &'a OpenedDiff, path: &str) -> &'a FileChange {
    opened
        .files
        .iter()
        .find(|f| f.display_path() == path)
        .unwrap_or_else(|| panic!("{path} in the diff"))
}

fn state_of(core: &Core, review: Option<&str>, change: &FileChange) -> ViewedState {
    core.viewed_states(review, std::slice::from_ref(change))
        .unwrap()[0]
}

fn count(core: &Core, sql: &str) -> i64 {
    core.store
        .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
        .unwrap()
}

fn exec(core: &Core, sql: &str, args: impl rusqlite::Params) {
    core.store
        .write(|tx| {
            tx.execute(sql, args)?;
            Ok(())
        })
        .unwrap();
}

/// `(kind, review_id, actor kind, payload)` of every event, oldest first.
fn events(core: &Core) -> Vec<(String, Option<String>, ActorKind, serde_json::Value)> {
    core.store
        .read(|c| events_since(c, 0, &EventFilter::default(), 10_000))
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                e.kind.as_str().to_owned(),
                e.review_id,
                e.actor.kind,
                e.payload,
            )
        })
        .collect()
}

fn kinds(core: &Core) -> Vec<String> {
    events(core).into_iter().map(|e| e.0).collect()
}

fn session(id: &str, pid: Option<i32>) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        client_name: "claude-code".into(),
        client_version: Some("2.1.0".into()),
        owner_pid: pid,
        cwd: Some("/tmp/work".into()),
    }
}

/// A pid that belonged to a process that has exited (and was reaped).
fn dead_pid() -> i32 {
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id() as i32;
    child.wait().unwrap();
    pid
}

fn far_future() -> i64 {
    polygloss_core::store::events::now_ms() + 3_600_000
}

// ---------------------------------------------------------------------------
// Viewed (design §9, ADR-0022)
// ---------------------------------------------------------------------------

#[test]
fn viewed_carries_over_when_blobs_unchanged() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let first = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let a1 = file(&first, "a.txt").clone();
    let b1 = file(&first, "b.txt").clone();

    core.set_viewed(Some(&first.review_id), &a1, true).unwrap();
    assert_eq!(
        core.viewed_states(Some(&first.review_id), &first.files)
            .unwrap(),
        [ViewedState::Viewed, ViewedState::NotViewed]
    );

    // A new iteration that changes only b.txt keeps a.txt viewed.
    commit_on_feature(&repo, &[("b.txt", b"bee\nbuzz\n")], "f2");
    let second = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    assert_eq!(second.iteration.as_ref().unwrap().seq, 2);
    assert_eq!(file(&second, "a.txt"), &a1);
    assert_ne!(file(&second, "b.txt").new_blob, b1.new_blob);
    assert_eq!(
        state_of(&core, Some(&second.review_id), file(&second, "a.txt")),
        ViewedState::Viewed
    );

    // The scope is global (OQ-8): another review with the same change sees it,
    // and so does a caller without a review.
    let other = core
        .open(&req(repo.path(), compare("main", "feature~1")))
        .unwrap();
    assert_ne!(other.review_id, first.review_id);
    assert_eq!(
        state_of(&core, Some(&other.review_id), file(&other, "a.txt")),
        ViewedState::Viewed
    );
    assert_eq!(state_of(&core, None, &a1), ViewedState::Viewed);
}

#[test]
fn viewed_clears_when_new_blob_changes() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let first = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    core.set_viewed(Some(&first.review_id), file(&first, "a.txt"), true)
        .unwrap();

    commit_on_feature(&repo, &[("a.txt", b"one\n2\n3\n")], "f2");
    let second = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let a2 = file(&second, "a.txt");
    assert_ne!(a2.new_blob, file(&first, "a.txt").new_blob);

    assert_ne!(
        state_of(&core, Some(&second.review_id), a2),
        ViewedState::Viewed
    );
    assert_eq!(state_of(&core, None, a2), ViewedState::NotViewed);
    // Unmarking the old key clears it for good.
    core.set_viewed(Some(&first.review_id), file(&first, "a.txt"), false)
        .unwrap();
    assert_eq!(
        state_of(&core, None, file(&first, "a.txt")),
        ViewedState::NotViewed
    );
    assert_eq!(count(&core, "SELECT count(*) FROM viewed_files"), 0);
}

#[test]
fn changed_since_viewed_rule() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let first = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = first.review_id.as_str();
    core.set_viewed(Some(review), file(&first, "a.txt"), true)
        .unwrap();

    commit_on_feature(&repo, &[("a.txt", b"one\n2\n3\n")], "f2");
    let second = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let a2 = file(&second, "a.txt").clone();

    // Same review, same path, a different blob pair: changed since viewed.
    assert_eq!(
        state_of(&core, Some(review), &a2),
        ViewedState::ChangedSinceViewed
    );
    // Not in a review that never marked the path.
    let other = core
        .open(&req(
            repo.path(),
            Source::Commit {
                rev: "feature".into(),
            },
        ))
        .unwrap();
    assert_ne!(other.review_id, review);
    assert_eq!(
        state_of(&core, Some(&other.review_id), &a2),
        ViewedState::NotViewed
    );
    // Other paths of the review are unaffected.
    assert_eq!(
        state_of(&core, Some(review), file(&second, "b.txt")),
        ViewedState::NotViewed
    );

    // Marking the new pair viewed wins over the badge.
    core.set_viewed(Some(review), &a2, true).unwrap();
    assert_eq!(state_of(&core, Some(review), &a2), ViewedState::Viewed);
    // Unmarking it is an explicit "not viewed": the badge does not come back,
    // while the older pair stays viewed globally.
    core.set_viewed(Some(review), &a2, false).unwrap();
    assert_eq!(state_of(&core, Some(review), &a2), ViewedState::NotViewed);
    assert_eq!(
        state_of(&core, Some(review), file(&first, "a.txt")),
        ViewedState::Viewed
    );
}

#[test]
fn viewed_added_file_uses_zero_old_blob() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let b = file(&opened, "b.txt");
    assert!(b.old_blob.is_zero());

    core.set_viewed(Some(&opened.review_id), b, true).unwrap();

    let row: (String, String, String, Option<String>) = core
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT path, old_blob, new_blob, review_id FROM viewed_files",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .unwrap();
    assert_eq!(row.0, "b.txt");
    assert_eq!(row.1, Oid::zero(ObjectFormat::Sha1).as_str());
    assert_eq!(row.2, b.new_blob.as_str());
    assert_eq!(row.3.as_deref(), Some(opened.review_id.as_str()));

    let ev = events(&core);
    let last = ev.last().unwrap();
    assert_eq!(last.0, "viewed.changed");
    assert_eq!(last.1.as_deref(), Some(opened.review_id.as_str()));
    assert_eq!(last.2, ActorKind::Human);
    assert_eq!(last.3["path"], "b.txt");
    assert_eq!(last.3["old_blob"], Oid::zero(ObjectFormat::Sha1).as_str());
    assert_eq!(last.3["new_blob"], b.new_blob.as_str());
    assert_eq!(last.3["viewed"], true);

    // Setting the same state again changes nothing and records nothing.
    let before = ev.len();
    core.set_viewed(Some(&opened.review_id), b, true).unwrap();
    assert_eq!(events(&core).len(), before);
}

#[test]
fn viewed_never_creates_iteration() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    repo.write("a.txt", b"one\nTWO\nthree\n");
    let core = core();
    let opened = core
        .open(&req(repo.path(), Source::Live { since: Since::Head }))
        .unwrap();
    assert!(opened.iteration.is_none());
    let a = file(&opened, "a.txt");

    core.set_viewed(Some(&opened.review_id), a, true).unwrap();

    assert_eq!(
        state_of(&core, Some(&opened.review_id), a),
        ViewedState::Viewed
    );
    assert!(core.iterations(&opened.review_id).unwrap().is_empty());
    assert_eq!(count(&core, "SELECT count(*) FROM iterations"), 0);
    assert_eq!(count(&core, "SELECT count(*) FROM diffs"), 0);
    assert!(!kinds(&core).iter().any(|k| k == "iteration.created"));
}

#[test]
fn set_viewed_on_unknown_review_is_not_found() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let err = core
        .set_viewed(Some("nope"), file(&opened, "a.txt"), true)
        .unwrap_err();
    assert!(matches!(err, CoreError::NotFound { .. }), "{err:?}");
    // Without a review the key is still recorded (review_id NULL).
    core.set_viewed(None, file(&opened, "a.txt"), true).unwrap();
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM viewed_files WHERE review_id IS NULL"
        ),
        1
    );
}

#[test]
fn viewed_survives_review_prune_as_global_key() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let a = file(&opened, "a.txt").clone();
    core.set_viewed(Some(&opened.review_id), &a, true).unwrap();
    core.prune_review(&opened.review_id).unwrap();
    assert_eq!(state_of(&core, None, &a), ViewedState::Viewed);
}

// ---------------------------------------------------------------------------
// View state (design §7.2 `view_state`, §11.12)
// ---------------------------------------------------------------------------

fn sample_view_state() -> ViewState {
    ViewState {
        v: 1,
        scroll_anchor: Some(ScrollAnchorState {
            path: "src/lib.rs".into(),
            side: Side::New,
            line: 42,
        }),
        collapsed: vec!["Cargo.lock".into()],
        expanded: BTreeMap::from([("src/lib.rs".into(), vec![[1, 20], [90, 110]])]),
        layout: Some(Layout::Unified),
        tree_expanded: Some(vec!["src".into(), "src/review".into()]),
        composer: BTreeMap::from([("line:src/lib.rs:new:42".into(), "draft text".into())]),
        threads_panel: Some(true),
    }
}

#[test]
fn view_state_roundtrip_v1() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    assert_eq!(core.load_view_state(&opened.diff_id).unwrap(), None);

    let state = sample_view_state();
    core.save_view_state(&opened.diff_id, &state).unwrap();
    assert_eq!(
        core.load_view_state(&opened.diff_id).unwrap(),
        Some(state.clone())
    );

    // The stored JSON is the §7.2 v1 shape.
    let json: String = core
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT state_json FROM view_state WHERE diff_id = ?1",
                [opened.diff_id.as_str()],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "v": 1,
            "scroll_anchor": { "path": "src/lib.rs", "side": "new", "line": 42 },
            "collapsed": ["Cargo.lock"],
            "expanded": { "src/lib.rs": [[1, 20], [90, 110]] },
            "layout": "unified",
            "tree_expanded": ["src", "src/review"],
            "composer": { "line:src/lib.rs:new:42": "draft text" },
            "threads_panel": true
        })
    );

    // Saving again replaces it; view state records no event.
    let mut next = state;
    next.layout = None;
    next.scroll_anchor = None;
    core.save_view_state(&opened.diff_id, &next).unwrap();
    assert_eq!(core.load_view_state(&opened.diff_id).unwrap(), Some(next));
    assert_eq!(count(&core, "SELECT count(*) FROM view_state"), 1);
    assert_eq!(kinds(&core), ["review.created", "iteration.created"]);

    // Missing fields default; unknown fields are ignored.
    exec(
        &core,
        "UPDATE view_state SET state_json = ?1",
        [r#"{"v":1,"layout":"split","future":true}"#],
    );
    let partial = core.load_view_state(&opened.diff_id).unwrap().unwrap();
    assert_eq!(partial.layout, Some(Layout::Split));
    assert!(partial.collapsed.is_empty() && partial.scroll_anchor.is_none());
    // A state saved without the tree's expansion (T3.2's layout-only write)
    // says nothing about it, unlike an empty list (every folder collapsed).
    assert_eq!(partial.tree_expanded, None);
    exec(
        &core,
        "UPDATE view_state SET state_json = ?1",
        [r#"{"v":1,"tree_expanded":[]}"#],
    );
    let collapsed = core.load_view_state(&opened.diff_id).unwrap().unwrap();
    assert_eq!(collapsed.tree_expanded, Some(Vec::new()));
    // `None` is left out of the JSON.
    let mut unsaved = sample_view_state();
    unsaved.tree_expanded = None;
    core.save_view_state(&opened.diff_id, &unsaved).unwrap();
    let json: String = core
        .store
        .read(|c| Ok(c.query_row("SELECT state_json FROM view_state", [], |r| r.get(0))?))
        .unwrap();
    assert!(!json.contains("tree_expanded"), "{json}");
    assert_eq!(
        core.load_view_state(&opened.diff_id).unwrap(),
        Some(unsaved)
    );
}

/// The stored JSON of the only view state.
fn stored_json(core: &Core) -> serde_json::Value {
    let json: String = core
        .store
        .read(|c| Ok(c.query_row("SELECT state_json FROM view_state", [], |r| r.get(0))?))
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

#[test]
fn view_state_threads_panel_is_optional_and_round_trips() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();

    // Never chosen: left out of the JSON (an older build reads it unchanged).
    core.save_view_state(&opened.diff_id, &ViewState::default())
        .unwrap();
    let json = stored_json(&core);
    assert_eq!(json.get("threads_panel"), None, "{json}");
    assert_eq!(
        core.load_view_state(&opened.diff_id)
            .unwrap()
            .unwrap()
            .threads_panel,
        None
    );

    // Shown or hidden: a boolean (design §7.2), version still 1.
    for shown in [true, false] {
        let state = ViewState {
            threads_panel: Some(shown),
            ..ViewState::default()
        };
        core.save_view_state(&opened.diff_id, &state).unwrap();
        let json = stored_json(&core);
        assert_eq!(json["threads_panel"], serde_json::json!(shown), "{json}");
        assert_eq!(json["v"], serde_json::json!(1));
        assert_eq!(core.load_view_state(&opened.diff_id).unwrap(), Some(state));
    }

    // A state written before M6 has no key: never chosen.
    exec(
        &core,
        "UPDATE view_state SET state_json = ?1",
        [r#"{"v":1,"layout":"split"}"#],
    );
    let old = core.load_view_state(&opened.diff_id).unwrap().unwrap();
    assert_eq!((old.layout, old.threads_panel), (Some(Layout::Split), None));
}

#[test]
fn view_state_unknown_version_ignored() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    core.save_view_state(&opened.diff_id, &sample_view_state())
        .unwrap();

    for json in [
        r#"{"v":2,"layout":"split"}"#,
        r#"{"layout":"split"}"#,
        r#"{"v":1,"layout":"diagonal"}"#,
        "not json",
    ] {
        exec(&core, "UPDATE view_state SET state_json = ?1", [json]);
        assert_eq!(
            core.load_view_state(&opened.diff_id).unwrap(),
            None,
            "{json}"
        );
    }

    // A state saved with another `v` is written as v1 (this build's shape).
    let mut other = sample_view_state();
    other.v = 7;
    core.save_view_state(&opened.diff_id, &other).unwrap();
    assert_eq!(
        core.load_view_state(&opened.diff_id).unwrap(),
        Some(sample_view_state())
    );
}

#[test]
fn view_state_for_unstored_diff() {
    let _sb = Sandbox::isolate();
    let core = core();
    let id = DiffId::parse(&"ab".repeat(32)).unwrap();
    assert_eq!(core.load_view_state(&id).unwrap(), None);
    let err = core.save_view_state(&id, &sample_view_state()).unwrap_err();
    assert!(
        matches!(err, CoreError::NotFound { what: "diff", .. }),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// Sessions, assignments (design §16.3, §16.4, OQ-32)
// ---------------------------------------------------------------------------

#[test]
fn session_owner_pid_links_canonical_id() {
    let _sb = Sandbox::isolate();
    let core = core();

    assert_eq!(
        core.upsert_session(&session("s1", Some(4242))).unwrap(),
        "s1"
    );
    // After /clear the hooks get a new id with the same owner process.
    assert_eq!(
        core.upsert_session(&session("s2", Some(4242))).unwrap(),
        "s1"
    );
    // A third id of that process links to the root, not to s2.
    assert_eq!(
        core.upsert_session(&session("s3", Some(4242))).unwrap(),
        "s1"
    );
    assert_eq!(core.canonical_session("s2").unwrap(), "s1");
    assert_eq!(core.canonical_session("s3").unwrap(), "s1");
    assert_eq!(core.canonical_session("s1").unwrap(), "s1");
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM sessions WHERE canonical_id = 's1'"
        ),
        2
    );

    // Another process, or no pid at all, stays separate.
    assert_eq!(core.upsert_session(&session("t1", Some(7))).unwrap(), "t1");
    assert_eq!(core.upsert_session(&session("u1", None)).unwrap(), "u1");
    assert_eq!(core.upsert_session(&session("u2", None)).unwrap(), "u2");

    // Seeing s1 again updates it and keeps it canonical.
    let mut again = session("s1", Some(4242));
    again.client_version = Some("2.2.0".into());
    assert_eq!(core.upsert_session(&again).unwrap(), "s1");
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM sessions WHERE id = 's1' AND client_version = '2.2.0' AND canonical_id IS NULL"
        ),
        1
    );
    // Seeing s2 again keeps its link.
    assert_eq!(
        core.upsert_session(&session("s2", Some(4242))).unwrap(),
        "s1"
    );

    // An unknown id is its own canonical id.
    assert_eq!(core.canonical_session("never-seen").unwrap(), "never-seen");
}

#[test]
fn assign_latest_opener_wins() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = opened.review_id.as_str();
    core.upsert_session(&session("s1", Some(1))).unwrap();
    core.upsert_session(&session("s2", Some(2))).unwrap();

    core.assign_review(review, "s1", AssignedBy::OpenDiff)
        .unwrap();
    assert_eq!(core.assigned_open_reviews("s1").unwrap(), [review]);
    core.assign_review(review, "s2", AssignedBy::OpenDiff)
        .unwrap();
    assert!(core.assigned_open_reviews("s1").unwrap().is_empty());
    assert_eq!(core.assigned_open_reviews("s2").unwrap(), [review]);
    // The same assignment again records nothing.
    core.assign_review(review, "s2", AssignedBy::OpenDiff)
        .unwrap();

    let assigned: Vec<_> = events(&core)
        .into_iter()
        .filter(|e| e.0 == "review.assigned")
        .collect();
    assert_eq!(assigned.len(), 2);
    assert_eq!(assigned[0].3["session_id"], "s1");
    assert_eq!(assigned[1].3["session_id"], "s2");
    assert_eq!(assigned[1].1.as_deref(), Some(review));
    assert_eq!(assigned[1].2, ActorKind::Agent);

    // Archived or approved reviews are no longer open for the waiter.
    core.archive_review(review, &Actor::human()).unwrap();
    assert!(core.assigned_open_reviews("s2").unwrap().is_empty());
    core.open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    assert_eq!(core.assigned_open_reviews("s2").unwrap(), [review]);
    exec(
        &core,
        "UPDATE reviews SET status = 'approved' WHERE id = ?1",
        [review],
    );
    assert!(core.assigned_open_reviews("s2").unwrap().is_empty());

    // Unknown review or session.
    assert!(matches!(
        core.assign_review("nope", "s1", AssignedBy::Agent),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        core.assign_review(review, "nobody", AssignedBy::Agent),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn assignment_follows_canonical_session() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    core.upsert_session(&session("s1", Some(99))).unwrap();
    core.upsert_session(&session("s2", Some(99))).unwrap();

    // Assigning through the drifted id stores the canonical one.
    core.assign_review(&opened.review_id, "s2", AssignedBy::OpenDiff)
        .unwrap();
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM review_assignments WHERE session_id = 's1'"
        ),
        1
    );
    // The waiter under either id finds it.
    assert_eq!(
        core.assigned_open_reviews("s1").unwrap(),
        [opened.review_id.as_str()]
    );
    assert_eq!(
        core.assigned_open_reviews("s2").unwrap(),
        [opened.review_id.as_str()]
    );
}

#[test]
fn human_reassign_records_assigned_by_human() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = opened.review_id.as_str();
    core.upsert_session(&session("s1", Some(1))).unwrap();
    core.upsert_session(&session("s2", Some(2))).unwrap();
    core.assign_review(review, "s1", AssignedBy::OpenDiff)
        .unwrap();

    core.assign_review(review, "s2", AssignedBy::Human).unwrap();

    let (session_id, by): (String, String) = core
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT session_id, assigned_by FROM review_assignments WHERE review_id = ?1",
                [review],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!((session_id.as_str(), by.as_str()), ("s2", "human"));
    let last = events(&core).pop().unwrap();
    assert_eq!(last.0, "review.assigned");
    assert_eq!(last.2, ActorKind::Human);
    assert_eq!(last.3["session_id"], "s2");
    assert_eq!(last.3["assigned_by"], "human");

    // Agent-driven reassignment records `agent`.
    core.assign_review(review, "s1", AssignedBy::Agent).unwrap();
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM review_assignments WHERE assigned_by = 'agent' AND session_id = 's1'"
        ),
        1
    );
}

#[test]
fn recent_sessions_filters_by_last_seen() {
    let _sb = Sandbox::isolate();
    let core = core();
    core.upsert_session(&session("old", Some(1))).unwrap();
    core.upsert_session(&session("new", Some(2))).unwrap();
    core.upsert_session(&session("new-alias", Some(2))).unwrap();
    core.upsert_session(&session("mid", None)).unwrap();
    exec(
        &core,
        "UPDATE sessions SET last_seen_at = 1000 WHERE id = 'old'",
        [],
    );
    exec(
        &core,
        "UPDATE sessions SET last_seen_at = 5000 WHERE id = 'mid'",
        [],
    );
    exec(
        &core,
        "UPDATE sessions SET last_seen_at = 2000 WHERE id = 'new'",
        [],
    );
    exec(
        &core,
        "UPDATE sessions SET last_seen_at = 9000 WHERE id = 'new-alias'",
        [],
    );

    let recent = core.recent_sessions(3000).unwrap();
    let ids: Vec<&str> = recent.iter().map(|s| s.id.as_str()).collect();
    // Canonical sessions only (an alias's activity counts for its root), most
    // recently seen first.
    assert_eq!(ids, ["new", "mid"]);
    assert_eq!(recent[0].client_name, "claude-code");
    assert_eq!(recent[0].owner_pid, Some(2));
    assert_eq!(recent[0].cwd.as_deref(), Some(Path::new("/tmp/work")));

    assert_eq!(core.recent_sessions(0).unwrap().len(), 3);
    assert!(core.recent_sessions(10_000).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Waiters (design §16.3)
// ---------------------------------------------------------------------------

#[test]
fn waiter_replacement_returns_previous_pid() {
    let _sb = Sandbox::isolate();
    let core = core();
    core.upsert_session(&session("s1", Some(50))).unwrap();
    core.upsert_session(&session("s2", Some(50))).unwrap();
    let deadline = far_future();

    assert_eq!(core.register_waiter("s1", 101, deadline).unwrap(), None);
    let replaced = core.register_waiter("s1", 102, deadline).unwrap().unwrap();
    assert_eq!(replaced.pid, 101);
    let now = polygloss_core::store::events::now_ms();
    assert!(replaced.started_at <= now && replaced.started_at > now - 60_000);
    // The same pid again replaces nothing.
    assert_eq!(core.register_waiter("s1", 102, deadline).unwrap(), None);
    // A waiter under a drifted id replaces the canonical session's waiter.
    assert_eq!(
        core.register_waiter("s2", 103, deadline)
            .unwrap()
            .map(|w| w.pid),
        Some(102)
    );
    assert_eq!(count(&core, "SELECT count(*) FROM waiters"), 1);

    // An old waiter exiting does not remove its replacement.
    core.remove_waiter("s1", 102).unwrap();
    assert_eq!(count(&core, "SELECT pid FROM waiters"), 103);
    core.remove_waiter("s2", 103).unwrap();
    assert_eq!(count(&core, "SELECT count(*) FROM waiters"), 0);

    assert!(matches!(
        core.register_waiter("nobody", 1, deadline),
        Err(CoreError::NotFound { .. })
    ));
}

/// A waiter row whose pid now belongs to a process that started after the
/// waiter registered (PID reuse) is not a live waiter (T5.9 #5).
#[test]
fn live_waiter_ignores_a_reused_pid() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = opened.review_id.as_str();
    core.upsert_session(&session("s1", None)).unwrap();
    core.assign_review(review, "s1", AssignedBy::OpenDiff)
        .unwrap();
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    core.register_waiter("s1", pid, far_future()).unwrap();
    assert_eq!(
        core.live_waiter_for_review(review).unwrap(),
        Some(("s1".to_owned(), pid))
    );
    // The row says the waiter registered a minute before this process began.
    exec(
        &core,
        "UPDATE waiters SET started_at = started_at - 60000 WHERE pid = ?1",
        params![pid],
    );
    assert_eq!(core.live_waiter_for_review(review).unwrap(), None);
    child.kill().unwrap();
    child.wait().unwrap();
}

/// The seq since which a review has been assigned to a session (T5.9 #6):
/// wake-ups for submissions before it belong to whoever had the review then.
#[test]
fn assignment_seq_is_where_the_current_assignment_began() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = opened.review_id.as_str();
    for s in ["a", "b", "b-drift"] {
        core.upsert_session(&session(s, None)).unwrap();
    }
    assert_eq!(core.assignment_seq(review, "a").unwrap(), None);
    core.assign_review(review, "a", AssignedBy::OpenDiff)
        .unwrap();
    let a_seq = latest_seq(&core);
    assert_eq!(core.assignment_seq(review, "a").unwrap(), Some(a_seq));
    assert_eq!(core.assignment_seq(review, "b").unwrap(), None);
    // Re-assigning the same session another way keeps the original point.
    core.assign_review(review, "a", AssignedBy::Human).unwrap();
    assert!(latest_seq(&core) > a_seq);
    assert_eq!(core.assignment_seq(review, "a").unwrap(), Some(a_seq));
    // Another session takes over: its point is its own assignment.
    core.assign_review(review, "b", AssignedBy::OpenDiff)
        .unwrap();
    let b_seq = latest_seq(&core);
    assert_eq!(core.assignment_seq(review, "b").unwrap(), Some(b_seq));
    assert_eq!(core.assignment_seq(review, "a").unwrap(), None);
    // And back to the first: a fresh point.
    core.assign_review(review, "a", AssignedBy::OpenDiff)
        .unwrap();
    let again = latest_seq(&core);
    assert_eq!(core.assignment_seq(review, "a").unwrap(), Some(again));
    assert!(matches!(
        core.assignment_seq("nope", "a"),
        Err(CoreError::NotFound { .. })
    ));
}

fn latest_seq(core: &Core) -> i64 {
    count(core, "SELECT MAX(seq) FROM events")
}

#[test]
fn live_waiter_ignores_dead_pid() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = opened.review_id.as_str();
    core.upsert_session(&session("s1", Some(60))).unwrap();
    core.upsert_session(&session("s2", Some(60))).unwrap();

    // Unassigned: no waiter.
    assert_eq!(core.live_waiter_for_review(review).unwrap(), None);
    core.assign_review(review, "s1", AssignedBy::OpenDiff)
        .unwrap();
    assert_eq!(core.live_waiter_for_review(review).unwrap(), None);

    core.register_waiter("s1", dead_pid(), far_future())
        .unwrap();
    assert_eq!(core.live_waiter_for_review(review).unwrap(), None);

    let me = std::process::id() as i32;
    core.register_waiter("s2", me, far_future()).unwrap();
    assert_eq!(
        core.live_waiter_for_review(review).unwrap(),
        Some(("s1".to_owned(), me))
    );

    // A waiter past its deadline has exited (or is about to).
    core.register_waiter("s1", me, 1).unwrap();
    assert_eq!(core.live_waiter_for_review(review).unwrap(), None);

    assert!(matches!(
        core.live_waiter_for_review("nope"),
        Err(CoreError::NotFound { .. })
    ));
}

// ---------------------------------------------------------------------------
// Seen, mute and review summaries (design §8.3, §11.2, §15.2 `list_reviews`, §17)
// ---------------------------------------------------------------------------

/// Inserts an open agent question on `review` (origin `diff`) with its published
/// root comment; returns the thread id.
fn agent_question(core: &Core, review: &str, diff: &DiffId, id: &str) -> String {
    let now = polygloss_core::store::events::now_ms();
    core.store
        .write(|tx| {
            tx.execute(
                "INSERT INTO threads (id, review_id, origin_diff_id, subject, kind, \
                   created_by_kind, created_by_name, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 'review', 'question', 'agent', 'claude-code', ?4, ?4)",
                params![id, review, diff.as_str(), now],
            )?;
            tx.execute(
                "INSERT INTO comments (id, thread_id, author_kind, author_name, body_md, \
                   published_at, created_at) \
                 VALUES (?1, ?2, 'agent', 'claude-code', 'Which one?', ?3, ?3)",
                params![format!("{id}-c1"), id, now],
            )?;
            Ok(())
        })
        .unwrap();
    id.to_owned()
}

fn human_comment(core: &Core, thread: &str, id: &str, published: bool) {
    let now = polygloss_core::store::events::now_ms();
    exec(
        core,
        "INSERT INTO comments (id, thread_id, author_kind, author_name, body_md, \
           published_at, created_at) VALUES (?1, ?2, 'human', 'you', 'This one.', ?3, ?4)",
        params![id, thread, published.then_some(now), now],
    );
}

fn summary_of(core: &Core, review: &str) -> polygloss_core::review::ReviewSummary {
    core.review_summaries(&ReviewFilter {
        include_archived: true,
        ..ReviewFilter::default()
    })
    .unwrap()
    .into_iter()
    .find(|s| s.review_id == review)
    .expect("review in summaries")
}

#[test]
fn summaries_awaiting_you_for_rereview_and_open_questions() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = opened.review_id.as_str();

    let s = summary_of(&core, review);
    assert!(!s.awaiting_you);
    assert_eq!((s.open_threads, s.open_questions), (0, 0));

    // An open agent question awaits you.
    let q = agent_question(&core, review, &opened.diff_id, "q1");
    let s = summary_of(&core, review);
    assert!(s.awaiting_you);
    assert_eq!((s.open_threads, s.open_questions), (1, 1));

    // A draft reply does not answer it; a submitted one does.
    human_comment(&core, &q, "h1", false);
    assert!(summary_of(&core, review).awaiting_you);
    human_comment(&core, &q, "h2", true);
    let s = summary_of(&core, review);
    assert!(!s.awaiting_you);
    assert_eq!(s.open_questions, 1, "still open until resolved");

    // Resolving a fresh question also ends the wait.
    let q2 = agent_question(&core, review, &opened.diff_id, "q2");
    assert!(summary_of(&core, review).awaiting_you);
    exec(
        &core,
        "UPDATE threads SET status = 'resolved' WHERE id = ?1",
        [q2.as_str()],
    );
    let s = summary_of(&core, review);
    assert!(!s.awaiting_you);
    assert_eq!((s.open_threads, s.open_questions), (1, 1));

    // Re-review requested awaits you, with its summary.
    exec(
        &core,
        "UPDATE reviews SET status = 'rereview_requested', rereview_summary = 'Fixed all', \
           rereview_at = 1234 WHERE id = ?1",
        [review],
    );
    let s = summary_of(&core, review);
    assert!(s.awaiting_you);
    assert_eq!(s.status, "rereview_requested");
    assert_eq!(s.rereview, Some(("Fixed all".to_owned(), 1234)));
}

/// The Dock badge (T3.17, design §17): unarchived reviews awaiting you,
/// muted ones included.
#[test]
fn awaiting_you_count_counts_rereview_and_open_questions_not_archived() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let a = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let b = core
        .open(&req(
            repo.path(),
            Source::Commit {
                rev: "feature".into(),
            },
        ))
        .unwrap();
    assert_eq!(core.awaiting_you_count().unwrap(), 0);

    let q = agent_question(&core, &a.review_id, &a.diff_id, "q1");
    assert_eq!(core.awaiting_you_count().unwrap(), 1);

    exec(
        &core,
        "UPDATE reviews SET status = 'rereview_requested', rereview_summary = 'Fixed' \
         WHERE id = ?1",
        [b.review_id.as_str()],
    );
    core.set_muted(&b.review_id, true).unwrap();
    assert_eq!(core.awaiting_you_count().unwrap(), 2, "muted still counts");

    human_comment(&core, &q, "h1", true);
    assert_eq!(core.awaiting_you_count().unwrap(), 1);

    core.archive_review(&b.review_id, &Actor::human()).unwrap();
    assert_eq!(
        core.awaiting_you_count().unwrap(),
        0,
        "archived is not counted"
    );
}

#[test]
fn summaries_hide_draft_threads_from_counts() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let now = polygloss_core::store::events::now_ms();
    core.store
        .write(|tx| {
            tx.execute(
                "INSERT INTO threads (id, review_id, origin_diff_id, subject, kind, \
                   created_by_kind, created_by_name, created_at, updated_at) \
                 VALUES ('d1', ?1, ?2, 'review', 'comment', 'human', 'you', ?3, ?3)",
                params![opened.review_id, opened.diff_id.as_str(), now],
            )?;
            Ok(())
        })
        .unwrap();
    human_comment(&core, "d1", "d1-c1", false);
    assert_eq!(summary_of(&core, &opened.review_id).open_threads, 0);
    exec(
        &core,
        "UPDATE comments SET published_at = 1 WHERE id = 'd1-c1'",
        [],
    );
    assert_eq!(summary_of(&core, &opened.review_id).open_threads, 1);
}

#[test]
fn summaries_viewed_counts_use_latest_iteration() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let first = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let review = first.review_id.clone();
    core.set_viewed(Some(&review), file(&first, "a.txt"), true)
        .unwrap();
    core.set_viewed(Some(&review), file(&first, "b.txt"), true)
        .unwrap();
    let s = summary_of(&core, &review);
    assert_eq!((s.viewed_done, s.viewed_total), (2, 2));
    assert_eq!(s.iterations, 1);
    assert_eq!(s.latest_diff_id.as_ref(), Some(&first.diff_id));

    // Iteration 2 changes b.txt and adds c.txt: only a.txt is still viewed.
    commit_on_feature(
        &repo,
        &[("b.txt", b"bee\nbuzz\n"), ("c.txt", b"sea\n")],
        "f2",
    );
    let second = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let s = summary_of(&core, &review);
    assert_eq!(s.iterations, 2);
    assert_eq!(s.latest_diff_id.as_ref(), Some(&second.diff_id));
    assert_eq!((s.viewed_done, s.viewed_total), (1, 3));

    // An unpinned live review has no iteration and nothing to count.
    let live = core
        .open(&req(repo.path(), Source::Live { since: Since::Head }))
        .unwrap();
    let s = summary_of(&core, &live.review_id);
    assert_eq!(s.kind, ReviewKind::Live);
    assert_eq!((s.iterations, s.latest_diff_id), (0, None));
    assert_eq!((s.viewed_done, s.viewed_total), (0, 0));
}

#[test]
fn summaries_sorted_by_updated_at_and_exclude_archived() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let a = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let mut labeled = req(
        repo.path(),
        Source::Commit {
            rev: "feature".into(),
        },
    );
    labeled.label = Some("PR #7".into());
    let b = core.open(&labeled).unwrap();
    let c = core
        .open(&req(repo.path(), compare("main", "feature~1")))
        .unwrap();
    exec(
        &core,
        "UPDATE reviews SET updated_at = 3000 WHERE id = ?1",
        [&a.review_id],
    );
    exec(
        &core,
        "UPDATE reviews SET updated_at = 1000 WHERE id = ?1",
        [&b.review_id],
    );
    exec(
        &core,
        "UPDATE reviews SET updated_at = 2000 WHERE id = ?1",
        [&c.review_id],
    );

    let all = core.review_summaries(&ReviewFilter::default()).unwrap();
    let ids: Vec<&str> = all.iter().map(|s| s.review_id.as_str()).collect();
    assert_eq!(ids, [&a.review_id, &c.review_id, &b.review_id]);
    let sb = &all[2];
    assert_eq!(sb.label.as_deref(), Some("PR #7"));
    assert_eq!(sb.kind, ReviewKind::Commit);
    assert_eq!(sb.key, b.review_key);
    assert_eq!(sb.repo_display, "repo");
    assert_eq!(sb.repo_path, repo.path());
    assert_eq!(sb.status, "open");
    assert_eq!(sb.updated_at, 1000);
    assert!(!sb.muted && sb.last_submission.is_none() && sb.assigned_session.is_none());

    core.archive_review(&c.review_id, &Actor::human()).unwrap();
    let ids: Vec<String> = core
        .review_summaries(&ReviewFilter::default())
        .unwrap()
        .into_iter()
        .map(|s| s.review_id)
        .collect();
    assert_eq!(ids, [a.review_id.clone(), b.review_id.clone()]);
    let with_archived = core
        .review_summaries(&ReviewFilter {
            include_archived: true,
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(with_archived.len(), 3);

    // Limit and keyset paging continue after the last row.
    let page1 = core
        .review_summaries(&ReviewFilter {
            limit: Some(1),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(page1.len(), 1);
    let page2 = core
        .review_summaries(&ReviewFilter {
            after: Some(page1[0].cursor()),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(
        page2
            .iter()
            .map(|s| s.review_id.as_str())
            .collect::<Vec<_>>(),
        [b.review_id.as_str()]
    );
}

#[test]
fn summaries_filters_submission_assignment_and_mute() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let other_repo = feature_repo();
    let core = core();
    let a = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let b = core
        .open(&req(other_repo.path(), compare("main", "feature")))
        .unwrap();

    // Last submission: the newest one.
    let it = a.iteration.as_ref().unwrap().id;
    for (id, verdict, at) in [("sub1", "request_changes", 10), ("sub2", "approve", 20)] {
        exec(
            &core,
            "INSERT INTO review_submissions (id, review_id, iteration_id, verdict, summary_md, \
               comment_count, submitted_at) VALUES (?1, ?2, ?3, ?4, 'Looks good', 0, ?5)",
            params![id, a.review_id, it, verdict, at],
        );
    }
    exec(
        &core,
        "UPDATE reviews SET status = 'approved' WHERE id = ?1",
        [&a.review_id],
    );
    let s = summary_of(&core, &a.review_id);
    let last = s.last_submission.unwrap();
    assert_eq!(
        (
            last.submission_id.as_str(),
            last.verdict,
            last.summary_md.as_str(),
            last.at
        ),
        ("sub2", Verdict::Approve, "Looks good", 20)
    );
    assert_eq!(
        serde_json::to_value(&last).unwrap()["verdict"],
        "approve",
        "the verdict serializes as its snake_case name"
    );

    // Repo, status and assignment filters.
    let by_repo = core
        .review_summaries(&ReviewFilter {
            repo_common_dir: Some(other_repo.path().join(".git")),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(by_repo.len(), 1);
    assert_eq!(by_repo[0].review_id, b.review_id);
    let approved = core
        .review_summaries(&ReviewFilter {
            status: Some("approved".into()),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0].review_id, a.review_id);

    core.upsert_session(&session("s1", Some(5))).unwrap();
    core.upsert_session(&session("s1b", Some(5))).unwrap();
    core.assign_review(&b.review_id, "s1", AssignedBy::OpenDiff)
        .unwrap();
    let mine = core
        .review_summaries(&ReviewFilter {
            assigned_session: Some("s1b".into()),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].review_id, b.review_id);
    assert_eq!(mine[0].assigned_session.as_deref(), Some("s1"));

    // One review by id (T3.10's "Reply in <review>"), archived ones too.
    let one = core
        .review_summaries(&ReviewFilter {
            review_id: Some(b.review_id.clone()),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].review_id, b.review_id);
    assert_eq!(
        core.review_summary(&b.review_id)
            .unwrap()
            .map(|s| s.review_id),
        Some(b.review_id.clone())
    );
    core.archive_review(&a.review_id, &Actor::human()).unwrap();
    assert_eq!(
        core.review_summary(&a.review_id)
            .unwrap()
            .map(|s| s.review_id),
        Some(a.review_id.clone())
    );
    assert_eq!(core.review_summary("nope").unwrap(), None);

    // The assigned session (the submit dialog names it), by its canonical id.
    let assigned = core.assigned_session(&b.review_id).unwrap().unwrap();
    assert_eq!(
        (assigned.id.as_str(), assigned.client_name.as_str()),
        ("s1", "claude-code")
    );
    assert_eq!(core.assigned_session(&a.review_id).unwrap(), None);
    assert!(matches!(
        core.assigned_session("nope"),
        Err(CoreError::NotFound { .. })
    ));

    // Mute.
    core.set_muted(&b.review_id, true).unwrap();
    assert!(summary_of(&core, &b.review_id).muted);
    core.set_muted(&b.review_id, false).unwrap();
    assert!(!summary_of(&core, &b.review_id).muted);
    assert!(matches!(
        core.set_muted("nope", true),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn mark_seen_only_moves_forward() {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let seen = |core: &Core| {
        core.store
            .read(|c| {
                Ok(c.query_row(
                    "SELECT last_seen_seq FROM reviews WHERE id = ?1",
                    [&opened.review_id],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap()
    };
    core.mark_seen(&opened.review_id, 5).unwrap();
    assert_eq!(seen(&core), 5);
    core.mark_seen(&opened.review_id, 3).unwrap();
    assert_eq!(seen(&core), 5);
    core.mark_seen(&opened.review_id, 9).unwrap();
    assert_eq!(seen(&core), 9);
    assert!(matches!(
        core.mark_seen("nope", 1),
        Err(CoreError::NotFound { .. })
    ));
}

// ---------------------------------------------------------------------------
// Waiter support (T4.8, design §16.3)
// ---------------------------------------------------------------------------

#[test]
fn session_lookup_returns_the_stored_row() {
    let _sb = Sandbox::isolate();
    let core = core();
    assert_eq!(core.session("s1").unwrap(), None);
    core.upsert_session(&session("s1", Some(77))).unwrap();
    assert_eq!(core.session("s1").unwrap(), Some(session("s1", Some(77))));
}

#[test]
fn last_woken_seq_is_kept_on_the_canonical_session_and_only_moves_forward() {
    let _sb = Sandbox::isolate();
    let core = core();
    core.upsert_session(&session("s1", Some(50))).unwrap();
    core.upsert_session(&session("s2", Some(50))).unwrap();
    assert_eq!(core.last_woken_seq("s1").unwrap(), 0);
    // Unknown sessions have never been woken.
    assert_eq!(core.last_woken_seq("nobody").unwrap(), 0);

    // A wake under the drifted id is recorded on the canonical session.
    core.set_last_woken_seq("s2", 12).unwrap();
    assert_eq!(core.last_woken_seq("s1").unwrap(), 12);
    assert_eq!(core.last_woken_seq("s2").unwrap(), 12);
    assert_eq!(
        count(&core, "SELECT last_woken_seq FROM sessions WHERE id = 's1'"),
        12
    );
    core.set_last_woken_seq("s1", 7).unwrap();
    assert_eq!(core.last_woken_seq("s2").unwrap(), 12);
    assert!(matches!(
        core.set_last_woken_seq("nobody", 3),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn waiter_pid_reports_the_canonical_sessions_waiter() {
    let _sb = Sandbox::isolate();
    let core = core();
    core.upsert_session(&session("s1", Some(50))).unwrap();
    core.upsert_session(&session("s2", Some(50))).unwrap();
    assert_eq!(core.waiter_pid("s1").unwrap(), None);
    core.register_waiter("s2", 321, far_future()).unwrap();
    assert_eq!(core.waiter_pid("s1").unwrap(), Some(321));
    assert_eq!(core.waiter_pid("s2").unwrap(), Some(321));
    assert_eq!(core.waiter_pid("nobody").unwrap(), None);
}
