//! Carry-forward positions (T1.15, design §8.6, ADR-0010).
//!
//! Every test runs under `Sandbox::isolate()` (temp `HOME`, data dir and git
//! config). Only nextest (one process per test) is supported.

use std::path::Path;

use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, CARRY_FORWARD_ENGINE_VERSION, Core, NewThread, OpenRequest, OpenedDiff,
    PinnedBy, Position, PositionState, Subject, ThreadAnchor, ThreadKind, Viewer, position_of,
};
use polygloss_core::store::events::Actor;
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::{DiffId, ObjectFormat};
use polygloss_diff::Side;

const A_MAIN: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
const A_FEATURE: &str = "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\nl10\n";

fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
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

fn line(path: &str, side: Side, start_line: u32, line: u32) -> Subject {
    Subject::Line {
        path: path.into(),
        side,
        start_line,
        line,
    }
}

fn file(path: &str) -> Subject {
    Subject::File { path: path.into() }
}

fn at(state: PositionState, path: &str, side: Side, start_line: u32, line: u32) -> Position {
    Position {
        state,
        path: Some(path.into()),
        side: Some(side),
        start_line: Some(start_line),
        line: Some(line),
    }
}

fn absent() -> Position {
    Position {
        state: PositionState::Absent,
        path: None,
        side: None,
        start_line: None,
        line: None,
    }
}

fn with_feature(extra: &str) -> String {
    format!("{extra}{A_FEATURE}")
}

struct Env {
    _sb: Sandbox,
    repo: FixtureRepo,
    core: Core,
}

/// `main`: `a.txt` (10 lines), `b.txt`, `c.txt`. `feature` (checked out): line 5
/// of `a.txt` changed. Every open is the direct compare `main..feature`, so moving
/// either branch makes a new iteration of the same review.
fn env() -> Env {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", A_MAIN.as_bytes());
    repo.write("b.txt", b"bee\n");
    repo.write("c.txt", b"c1\nc2\nc3\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", A_FEATURE.as_bytes());
    repo.commit("f1");
    let core = Core::open_default().expect("open core in the sandbox");
    Env {
        _sb: sb,
        repo,
        core,
    }
}

impl Env {
    fn open(&self) -> OpenedDiff {
        self.core
            .open(&req(
                self.repo.path(),
                Source::Compare {
                    base: "main".into(),
                    head: "feature".into(),
                    mode: CompareMode::Direct,
                },
            ))
            .unwrap()
    }

    fn blobs(&self, opened: &OpenedDiff) -> BlobReader {
        BlobReader::open(&opened.repo).unwrap()
    }

    fn thread_on(&self, opened: &OpenedDiff, diff_id: &DiffId, subject: Subject) -> String {
        self.core
            .create_thread(
                &NewThread {
                    review_id: opened.review_id.clone(),
                    diff_id: diff_id.clone(),
                    subject,
                    kind: ThreadKind::Comment,
                    body_md: "look here".into(),
                    author: agent(),
                },
                &self.blobs(opened),
            )
            .unwrap()
    }

    fn thread(&self, opened: &OpenedDiff, subject: Subject) -> String {
        self.thread_on(opened, &opened.diff_id, subject)
    }

    fn anchor(&self, thread_id: &str) -> ThreadAnchor {
        self.core.thread(thread_id, Viewer::Human).unwrap().anchor
    }

    /// Commits `edit` on `feature` (checked out).
    fn commit_feature(&self, edit: impl FnOnce(&FixtureRepo)) {
        edit(&self.repo);
        self.repo.commit("feature edit");
    }

    /// Commits `edit` on `main`, then checks `feature` out again.
    fn commit_main(&self, edit: impl FnOnce(&FixtureRepo)) {
        self.repo.checkout("main");
        edit(&self.repo);
        self.repo.commit("main edit");
        self.repo.checkout("feature");
    }

    /// The thread's position in the current `main..feature` diff.
    fn position_now(&self, thread_id: &str) -> Position {
        let now = self.open();
        position_of(&self.anchor(thread_id), &now.files, &self.blobs(&now)).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.core
            .store
            .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
            .unwrap()
    }

    fn exec(&self, sql: &str) {
        self.core
            .store
            .write(|tx| {
                tx.execute(sql, [])?;
                Ok(())
            })
            .unwrap();
    }

    fn cached_row(&self, thread_id: &str, diff_id: &DiffId) -> (String, Option<u32>, i64) {
        self.core
            .store
            .read(|c| {
                Ok(c.query_row(
                    "SELECT state, line, engine_version FROM thread_positions \
                     WHERE thread_id = ?1 AND diff_id = ?2",
                    [thread_id, diff_id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .unwrap()
    }
}

#[test]
fn position_exact_when_blob_same() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let expected = at(PositionState::Exact, "a.txt", Side::New, 6, 7);

    // In its own diff, and after a commit that leaves a.txt alone.
    assert_eq!(e.position_now(&t), expected);
    e.commit_feature(|r| r.write("b.txt", b"changed\n"));
    let d2 = e.open();
    assert_ne!(d2.diff_id, d1.diff_id);
    assert_eq!(e.position_now(&t), expected);
}

#[test]
fn position_moved_when_lines_unchanged() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    e.commit_feature(|r| r.write("a.txt", with_feature("n1\nn2\n").as_bytes()));
    assert_eq!(
        e.position_now(&t),
        at(PositionState::Moved, "a.txt", Side::New, 8, 9)
    );
}

#[test]
fn position_moved_keeps_numbers_when_the_change_is_below() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 2, 3));
    e.commit_feature(|r| r.write("a.txt", format!("{A_FEATURE}l11\n").as_bytes()));
    // The blob changed, so the thread moved (§8.6), even to the same numbers.
    assert_eq!(
        e.position_now(&t),
        at(PositionState::Moved, "a.txt", Side::New, 2, 3)
    );
}

#[test]
fn position_outdated_when_anchored_line_changed() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let single = e.thread(&d1, line("a.txt", Side::New, 9, 9));
    e.commit_feature(|r| r.write("a.txt", A_FEATURE.replace("l7\n", "L7\n").as_bytes()));
    assert_eq!(e.position_now(&t).state, PositionState::Outdated);
    assert_eq!(e.position_now(&single).state, PositionState::Moved);

    // Lines inserted between the anchored lines break the range too.
    e.commit_feature(|r| r.write("a.txt", A_FEATURE.replace("l9\n", "l9\nnew\n").as_bytes()));
    let range = e.thread(&e.open(), line("a.txt", Side::New, 8, 9));
    e.commit_feature(|r| r.write("a.txt", A_FEATURE.replace("l8\n", "l8\nwedge\n").as_bytes()));
    assert_eq!(e.position_now(&range).state, PositionState::Outdated);
}

#[test]
fn position_outdated_placed_at_nearest_line() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));

    // Two lines above, and line 7 rewritten: the thread renders at the rewritten
    // line's new number.
    e.commit_feature(|r| {
        r.write(
            "a.txt",
            with_feature("n1\nn2\n").replace("l7\n", "L7\n").as_bytes(),
        )
    });
    assert_eq!(
        e.position_now(&t),
        at(PositionState::Outdated, "a.txt", Side::New, 9, 9)
    );

    // Both anchored lines deleted: the line after the gap (l8, now line 6).
    e.commit_feature(|r| {
        r.write(
            "a.txt",
            A_FEATURE.replace("l6\n", "").replace("l7\n", "").as_bytes(),
        )
    });
    assert_eq!(
        e.position_now(&t),
        at(PositionState::Outdated, "a.txt", Side::New, 6, 6)
    );
    // The original anchor is kept for the Outdated rendering.
    let anchor = e.anchor(&t);
    assert_eq!(anchor.subject, line("a.txt", Side::New, 6, 7));
    assert!(anchor.anchor_snippet.unwrap().contains("l6\nl7"));
}

#[test]
fn position_outdated_without_lines_when_the_file_has_none() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let no_lines = Position {
        state: PositionState::Outdated,
        path: Some("a.txt".into()),
        side: Some(Side::New),
        start_line: None,
        line: None,
    };
    e.commit_feature(|r| r.write("a.txt", b""));
    assert_eq!(e.position_now(&t), no_lines);
    e.commit_feature(|r| r.write("a.txt", b"\0binary now\n"));
    assert_eq!(e.position_now(&t), no_lines);
}

#[test]
fn position_follows_rename() {
    let e = env();
    let d1 = e.open();
    let new_side = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let old_side = e.thread(&d1, line("a.txt", Side::Old, 2, 3));
    let whole = e.thread(&d1, file("a.txt"));
    e.commit_feature(|r| {
        r.git(&["mv", "a.txt", "z.txt"]);
        r.write("z.txt", with_feature("n0\n").as_bytes());
    });
    let now = e.open();
    let renamed = now
        .files
        .iter()
        .find(|f| f.new_path.as_ref().is_some_and(|p| p.text == "z.txt"))
        .unwrap();
    assert_eq!(renamed.old_path.as_ref().unwrap().text, "a.txt");

    assert_eq!(
        e.position_now(&new_side),
        at(PositionState::Moved, "z.txt", Side::New, 7, 8)
    );
    assert_eq!(
        e.position_now(&old_side),
        at(PositionState::Exact, "z.txt", Side::Old, 2, 3)
    );
    assert_eq!(e.position_now(&whole).state, PositionState::Exact);
    assert_eq!(e.position_now(&whole).path.as_deref(), Some("z.txt"));
}

#[test]
fn position_absent_when_file_removed() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let whole = e.thread(&d1, file("a.txt"));
    // a.txt matches main again: it is no longer part of the diff.
    e.commit_feature(|r| r.write("a.txt", A_MAIN.as_bytes()));
    assert!(!e.open().files.iter().any(|f| f.display_path() == "a.txt"));
    assert_eq!(e.position_now(&t), absent());
    assert_eq!(e.position_now(&whole), absent());
}

#[test]
fn position_on_a_deleted_file() {
    let e = env();
    let d1 = e.open();
    let new_side = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let old_side = e.thread(&d1, line("a.txt", Side::Old, 2, 3));
    e.commit_feature(|r| {
        r.git(&["rm", "-q", "a.txt"]);
    });
    // The new side is gone; the old side is still the base blob.
    assert_eq!(e.position_now(&new_side), absent());
    assert_eq!(
        e.position_now(&old_side),
        at(PositionState::Exact, "a.txt", Side::Old, 2, 3)
    );

    // A thread on a deleted file stores its old path and follows it while the
    // file stays deleted.
    e.commit_feature(|r| {
        r.git(&["rm", "-q", "c.txt"]);
    });
    let d3 = e.open();
    let deleted = e.thread(&d3, line("c.txt", Side::Old, 2, 2));
    e.commit_feature(|r| r.write("b.txt", b"changed\n"));
    assert_eq!(
        e.position_now(&deleted),
        at(PositionState::Exact, "c.txt", Side::Old, 2, 2)
    );
}

#[test]
fn position_old_side_maps_against_current_base() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::Old, 2, 3));

    // The head moves but the base does not: still exact.
    e.commit_feature(|r| r.write("a.txt", with_feature("n1\n").as_bytes()));
    assert_eq!(
        e.position_now(&t),
        at(PositionState::Exact, "a.txt", Side::Old, 2, 3)
    );

    // The base gains a line above: the old-side thread moves with it.
    e.commit_main(|r| r.write("a.txt", format!("l0\n{A_MAIN}").as_bytes()));
    assert_eq!(
        e.position_now(&t),
        at(PositionState::Moved, "a.txt", Side::Old, 3, 4)
    );

    // An added file has no old side.
    e.commit_main(|r| {
        r.git(&["rm", "-q", "a.txt"]);
    });
    assert_eq!(e.position_now(&t), absent());
}

#[test]
fn position_file_subject_exact() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, file("a.txt"));
    e.commit_feature(|r| r.write("a.txt", b"rewritten entirely\n"));
    assert_eq!(
        e.position_now(&t),
        Position {
            state: PositionState::Exact,
            path: Some("a.txt".into()),
            side: None,
            start_line: None,
            line: None,
        }
    );
}

#[test]
fn position_review_subject_panel() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, Subject::Review);
    let panel = Position {
        state: PositionState::Exact,
        path: None,
        side: None,
        start_line: None,
        line: None,
    };
    e.commit_feature(|r| r.write("a.txt", A_MAIN.as_bytes()));
    assert_eq!(e.position_now(&t), panel);
    // Needs no file of the diff at all.
    let blobs = e.blobs(&d1);
    assert_eq!(position_of(&e.anchor(&t), &[], &blobs).unwrap(), panel);
}

#[test]
fn position_cache_hit_then_engine_version_bump_recomputes() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    e.commit_feature(|r| r.write("a.txt", with_feature("n1\nn2\n").as_bytes()));
    let d2 = e.open();
    let blobs = e.blobs(&d2);
    let moved = at(PositionState::Moved, "a.txt", Side::New, 8, 9);
    let ids = [t.clone()];

    let map = e
        .core
        .positions(&d2.diff_id, &d2.files, &ids, &blobs)
        .unwrap();
    assert_eq!(map[&t], moved);
    assert_eq!(
        e.cached_row(&t, &d2.diff_id),
        ("moved".into(), Some(9), CARRY_FORWARD_ENGINE_VERSION)
    );

    // A cache hit is served from the row: no files needed, tampering shows.
    e.exec("UPDATE thread_positions SET state = 'outdated', start_line = 1, line = 1");
    let hit = e.core.positions(&d2.diff_id, &[], &ids, &blobs).unwrap();
    assert_eq!(
        hit[&t],
        at(PositionState::Outdated, "a.txt", Side::New, 1, 1)
    );

    // A row from an older engine is recomputed and replaced.
    e.exec(&format!(
        "UPDATE thread_positions SET engine_version = {}",
        CARRY_FORWARD_ENGINE_VERSION - 1
    ));
    let again = e
        .core
        .positions(&d2.diff_id, &d2.files, &ids, &blobs)
        .unwrap();
    assert_eq!(again[&t], moved);
    assert_eq!(
        e.cached_row(&t, &d2.diff_id),
        ("moved".into(), Some(9), CARRY_FORWARD_ENGINE_VERSION)
    );

    // A row from a newer engine is not trusted, and not overwritten either.
    e.exec(&format!(
        "UPDATE thread_positions SET engine_version = {}, state = 'absent', path = NULL, \
         side = NULL, start_line = NULL, line = NULL",
        CARRY_FORWARD_ENGINE_VERSION + 1
    ));
    let newer = e
        .core
        .positions(&d2.diff_id, &d2.files, &ids, &blobs)
        .unwrap();
    assert_eq!(newer[&t], moved);
    assert_eq!(
        e.cached_row(&t, &d2.diff_id),
        ("absent".into(), None, CARRY_FORWARD_ENGINE_VERSION + 1)
    );
}

#[test]
fn positions_skip_unknown_threads_and_cache_every_state() {
    let e = env();
    let d1 = e.open();
    let gone = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let review = e.thread(&d1, Subject::Review);
    e.commit_feature(|r| r.write("a.txt", A_MAIN.as_bytes()));
    let d2 = e.open();
    let ids = [gone.clone(), "no-such-thread".to_owned(), review.clone()];
    let map = e
        .core
        .positions(&d2.diff_id, &d2.files, &ids, &e.blobs(&d2))
        .unwrap();
    assert_eq!(map.len(), 2);
    assert!(!map.contains_key("no-such-thread"));
    assert_eq!(map[&gone], absent());
    assert_eq!(map[&review].state, PositionState::Exact);

    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM thread_positions WHERE diff_id = '{}'",
            d2.diff_id.as_str()
        )),
        2
    );
    // Cached rows come back identical.
    let cached = e
        .core
        .positions(&d2.diff_id, &d2.files, &ids, &e.blobs(&d2))
        .unwrap();
    assert_eq!(cached, map);
}

#[test]
fn positions_in_the_origin_diff_are_exact_and_cached() {
    let e = env();
    let d1 = e.open();
    let t = e.thread(&d1, line("a.txt", Side::New, 6, 7));
    let map = e
        .core
        .positions(
            &d1.diff_id,
            &d1.files,
            std::slice::from_ref(&t),
            &e.blobs(&d1),
        )
        .unwrap();
    assert_eq!(map[&t], at(PositionState::Exact, "a.txt", Side::New, 6, 7));
    assert_eq!(
        e.count("SELECT count(*) FROM thread_positions WHERE state = 'exact'"),
        1
    );
}

#[test]
fn live_refresh_positions_without_pinning() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", A_MAIN.as_bytes());
    repo.commit("c1");
    repo.write("a.txt", A_FEATURE.as_bytes());
    let core = Core::open_default().unwrap();
    let live = || {
        core.open(&req(repo.path(), Source::Live { since: Since::Head }))
            .unwrap()
    };
    let opened = live();
    assert!(opened.iteration.is_none());

    // Commenting on a live diff pins it first (T3.10, T4.6).
    let it1 = core
        .pin_live_on_base(
            &opened.review_id,
            &opened.base,
            opened.live.as_ref().unwrap(),
            PinnedBy::Agent,
            &Actor::human(),
        )
        .unwrap();
    let t = core
        .create_thread(
            &NewThread {
                review_id: opened.review_id.clone(),
                diff_id: it1.diff_id.clone(),
                subject: line("a.txt", Side::New, 6, 7),
                kind: ThreadKind::Comment,
                body_md: "look here".into(),
                author: agent(),
            },
            &BlobReader::open(&opened.repo).unwrap(),
        )
        .unwrap();

    // The user keeps editing; the refreshed live diff is not pinned.
    repo.write("a.txt", with_feature("n1\n").as_bytes());
    let refreshed = live();
    assert_ne!(refreshed.diff_id, it1.diff_id);
    let blobs = BlobReader::open(&refreshed.repo)
        .unwrap()
        .with_scratch(&refreshed.live.as_ref().unwrap().scratch_objects)
        .unwrap();
    let map = core
        .positions(
            &refreshed.diff_id,
            &refreshed.files,
            std::slice::from_ref(&t),
            &blobs,
        )
        .unwrap();
    assert_eq!(map[&t], at(PositionState::Moved, "a.txt", Side::New, 7, 8));

    let count = |sql: &str| {
        core.store
            .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
            .unwrap()
    };
    assert_eq!(count("SELECT count(*) FROM iterations"), 1);
    assert_eq!(count("SELECT count(*) FROM diffs"), 1);
    assert_eq!(count("SELECT count(*) FROM thread_positions"), 0);
}

#[test]
fn position_serializes_state_snake_case() {
    let p = at(PositionState::Outdated, "a.txt", Side::Old, 3, 3);
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["state"], "outdated");
    assert_eq!(json["side"], "old");
    assert_eq!(serde_json::from_value::<Position>(json).unwrap(), p);
}
