//! Threads, comments, drafts, submission, resolve and suggestions (T1.13, design
//! §7.3, §8.1–§8.5, ADR-0011, ADR-0020, OQ-10, OQ-13, OQ-30).
//!
//! Every test runs under `Sandbox::isolate()` (temp `HOME`, data dir and git
//! config). Only nextest (one process per test) is supported.

use std::path::Path;

use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, CoreError, NewThread, OpenRequest, OpenedDiff, PinnedBy, Subject,
    SubmitDraft, ThreadFilter, ThreadKind, ThreadScope, ThreadStatus, Verdict, Viewer,
    parse_suggestions,
};
use polygloss_core::store::events::{Actor, ActorKind, EventFilter, events_since};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::{DiffId, ObjectFormat};
use polygloss_diff::Side;

fn core() -> Core {
    Core::open_default().expect("open core in the sandbox")
}

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

fn agent_named(name: &str) -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: name.into(),
        session_id: Some("s-1".into()),
    }
}

fn agent() -> Author {
    agent_named("claude-code")
}

fn agent_actor() -> Actor {
    Actor {
        kind: ActorKind::Agent,
        name: Some("claude-code".into()),
        session_id: Some("s-1".into()),
    }
}

fn human_actor() -> Actor {
    Actor {
        kind: ActorKind::Human,
        name: Some("you".into()),
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

fn compare(base: &str, head: &str) -> Source {
    Source::Compare {
        base: base.into(),
        head: head.into(),
        mode: CompareMode::ThreeDot,
    }
}

const A_MAIN: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
const A_FEATURE: &str = "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\nl10\n";

/// `main`: `a.txt` (10 lines), `c.txt`, `d.bin`, `old.txt`. `feature`: line 5 of
/// `a.txt` changed, `b.txt` added, `c.txt` deleted, `d.bin` changed, `old.txt`
/// renamed to `new.txt`.
fn review_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", A_MAIN.as_bytes());
    repo.write("c.txt", b"gone\nsoon\n");
    repo.write("d.bin", b"\0bin\n");
    repo.write("old.txt", b"x1\nx2\nx3\nx4\nx5\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", A_FEATURE.as_bytes());
    repo.write("b.txt", b"bee\nbuzz\n");
    std::fs::remove_file(repo.path().join("c.txt")).unwrap();
    repo.write("d.bin", b"\0bin2\n");
    std::fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
    repo.commit("f1");
    repo.checkout("main");
    repo
}

struct Env {
    _sb: Sandbox,
    repo: FixtureRepo,
    core: Core,
    opened: OpenedDiff,
    blobs: BlobReader,
}

fn env() -> Env {
    let sb = Sandbox::isolate();
    let repo = review_repo();
    let core = core();
    let opened = core
        .open(&req(repo.path(), compare("main", "feature")))
        .unwrap();
    let blobs = BlobReader::open(&opened.repo).unwrap();
    add_session(&core, "s-1");
    Env {
        _sb: sb,
        repo,
        core,
        opened,
        blobs,
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

fn new_thread(
    opened: &OpenedDiff,
    subject: Subject,
    kind: ThreadKind,
    body: &str,
    author: Author,
) -> NewThread {
    NewThread {
        review_id: opened.review_id.clone(),
        diff_id: opened.diff_id.clone(),
        subject,
        kind,
        body_md: body.into(),
        author,
    }
}

impl Env {
    fn human_thread(&self, subject: Subject, body: &str) -> String {
        self.core
            .create_thread(
                &new_thread(&self.opened, subject, ThreadKind::Comment, body, human()),
                &self.blobs,
            )
            .unwrap()
    }

    fn agent_thread(&self, kind: ThreadKind, subject: Subject, body: &str) -> String {
        self.core
            .create_thread(
                &new_thread(&self.opened, subject, kind, body, agent()),
                &self.blobs,
            )
            .unwrap()
    }

    fn review_scope(&self) -> ThreadScope {
        ThreadScope::Review(self.opened.review_id.clone())
    }

    fn count(&self, sql: &str) -> i64 {
        count(&self.core, sql)
    }

    fn text(&self, sql: &str) -> Option<String> {
        self.core
            .store
            .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, Option<String>>(0))?))
            .unwrap()
    }
}

/// Registers an agent session (T1.14's `upsert_session` does this in the app).
fn add_session(core: &Core, id: &str) {
    core.store
        .write(|tx| {
            tx.execute(
                "INSERT INTO sessions (id, client_name, first_seen_at, last_seen_at) \
                 VALUES (?1, 'claude-code', 1, 1)",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
}

fn count(core: &Core, sql: &str) -> i64 {
    core.store
        .read(|c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
        .unwrap()
}

/// `(seq, kind, thread_id, comment_id, payload, actor_name)` of every event after
/// `after`.
type Ev = (
    i64,
    String,
    Option<String>,
    Option<String>,
    serde_json::Value,
    Option<String>,
);

fn events_after(core: &Core, after: i64) -> Vec<Ev> {
    core.store
        .read(|c| events_since(c, after, &EventFilter::default(), 100_000))
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                e.seq,
                e.kind.as_str().to_owned(),
                e.thread_id,
                e.comment_id,
                e.payload,
                e.actor.name,
            )
        })
        .collect()
}

fn kinds(evs: &[Ev]) -> Vec<&str> {
    evs.iter().map(|e| e.1.as_str()).collect()
}

fn max_seq(core: &Core) -> i64 {
    count(core, "SELECT COALESCE(MAX(seq), 0) FROM events")
}

fn all() -> ThreadFilter {
    ThreadFilter::default()
}

#[test]
fn human_thread_is_draft_until_submit() {
    let e = env();
    let before = max_seq(&e.core);
    let t = e.human_thread(line("a.txt", Side::New, 5, 5), "why?");

    let view = e.core.thread(&t, Viewer::Human).unwrap();
    assert!(view.draft);
    assert_eq!(view.comments.len(), 1);
    assert!(view.comments[0].draft);
    assert_eq!(view.comments[0].published_at, None);
    assert_eq!(view.comments[0].body_md, "why?");
    assert_eq!(view.created_by.kind, AuthorKind::Human);
    assert_eq!(view.review_id.as_deref(), Some(e.opened.review_id.as_str()));
    assert_eq!(view.origin_diff_id, e.opened.diff_id);
    assert_eq!(
        view.origin_iteration_id,
        Some(e.opened.iteration.as_ref().unwrap().id)
    );
    assert_eq!(e.core.drafts_count(&e.opened.review_id).unwrap(), 1);
    assert_eq!(
        e.count("SELECT count(*) FROM comments WHERE published_at IS NULL"),
        1
    );
    // Only the app-internal draft event: no thread.created before submission.
    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["draft.changed"]);
    assert_eq!(evs[0].2.as_deref(), Some(t.as_str()));

    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();
    let view = e.core.thread(&t, Viewer::Human).unwrap();
    assert!(!view.draft);
    assert!(view.comments[0].published_at.is_some());
    assert_eq!(e.core.drafts_count(&e.opened.review_id).unwrap(), 0);
    let evs = events_after(&e.core, before);
    assert!(kinds(&evs).contains(&"thread.created"));
    assert_eq!(e.core.thread(&t, Viewer::Agent).unwrap().id, t);
}

#[test]
fn agent_viewer_never_sees_drafts_or_draft_roots() {
    let e = env();
    let draft_root = e.human_thread(line("a.txt", Side::New, 1, 2), "draft root");
    let agent_t = e.agent_thread(ThreadKind::Note, Subject::Review, "note");
    let draft_reply = e.core.reply(&agent_t, "draft reply", &human()).unwrap();

    let seen = e
        .core
        .threads(e.review_scope(), Viewer::Agent, &all())
        .unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].id, agent_t);
    assert_eq!(seen[0].comments.len(), 1);
    assert!(seen[0].comments.iter().all(|c| c.id != draft_reply));

    let by_diff = e
        .core
        .threads(
            ThreadScope::Diff(e.opened.diff_id.clone()),
            Viewer::Agent,
            &all(),
        )
        .unwrap();
    assert_eq!(
        by_diff.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        [agent_t.as_str()]
    );

    // A thread whose root is a draft does not exist for agents.
    assert!(matches!(
        e.core.thread(&draft_root, Viewer::Agent),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        e.core.reply(&draft_root, "hi", &agent()),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        e.core.set_resolved(&draft_root, true, &agent_actor(), None),
        Err(CoreError::NotFound { .. })
    ));
    // Nor does a draft comment.
    assert!(matches!(
        e.core.edit_comment(&draft_reply, "x", &agent()),
        Err(CoreError::NotFound { .. })
    ));

    // The human sees everything, drafts included.
    let mine = e
        .core
        .threads(e.review_scope(), Viewer::Human, &all())
        .unwrap();
    assert_eq!(mine.len(), 2);
    let agent_view = mine.iter().find(|t| t.id == agent_t).unwrap();
    assert_eq!(agent_view.comments.len(), 2);
    assert!(agent_view.comments[1].draft);
}

#[test]
fn submit_publishes_all_drafts_in_one_transaction() {
    let e = env();
    let agent_t = e.agent_thread(ThreadKind::Question, Subject::Review, "which one?");
    let t1 = e.human_thread(line("a.txt", Side::New, 5, 5), "one");
    let t2 = e.human_thread(
        Subject::File {
            path: "b.txt".into(),
        },
        "two",
    );
    let r1 = e.core.reply(&agent_t, "this one", &human()).unwrap();
    let r2 = e.core.reply(&t1, "and more", &human()).unwrap();
    e.core
        .save_submit_draft(&e.opened.review_id, "sum", Some(Verdict::Comment))
        .unwrap();
    let before = max_seq(&e.core);

    let sub = e
        .core
        .submit_review(
            &e.opened.review_id,
            Verdict::RequestChanges,
            "please fix",
            None,
        )
        .unwrap();

    assert_eq!(sub.comment_count, 4);
    assert_eq!(sub.verdict, Verdict::RequestChanges);
    assert_eq!(sub.summary_md, "please fix");
    assert_eq!(sub.review_id, e.opened.review_id);
    assert_eq!(&sub.iteration, e.opened.iteration.as_ref().unwrap());

    let evs = events_after(&e.core, before);
    assert_eq!(
        kinds(&evs),
        [
            "thread.created",
            "thread.created",
            "comment.created",
            "comment.created",
            "review.submitted"
        ]
    );
    // One transaction: nothing interleaves, so the seqs are contiguous.
    for pair in evs.windows(2) {
        assert_eq!(pair[1].0, pair[0].0 + 1);
    }
    assert_eq!(evs[0].2.as_deref(), Some(t1.as_str()));
    assert_eq!(evs[0].4["kind"], "comment");
    assert_eq!(evs[0].4["subject"], "line");
    assert_eq!(evs[1].2.as_deref(), Some(t2.as_str()));
    assert_eq!(evs[1].4["subject"], "file");
    assert_eq!(evs[2].3.as_deref(), Some(r1.as_str()));
    assert_eq!(evs[3].3.as_deref(), Some(r2.as_str()));
    assert_eq!(evs[4].4["submission_id"], sub.id.as_str());
    assert_eq!(evs[4].4["verdict"], "request_changes");
    assert_eq!(evs[4].0, sub.seq);

    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM comments WHERE submission_id = '{}' AND published_at IS NOT NULL",
            sub.id
        )),
        4
    );
    assert_eq!(
        e.count("SELECT count(*) FROM comments WHERE published_at IS NULL"),
        0
    );
    assert_eq!(e.count("SELECT count(*) FROM review_drafts"), 0);
    assert_eq!(
        e.count("SELECT comment_count FROM review_submissions"),
        4,
        "review_submissions row"
    );
    // The agent now sees every thread and comment.
    let seen = e
        .core
        .threads(e.review_scope(), Viewer::Agent, &all())
        .unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen.iter().map(|t| t.comments.len()).sum::<usize>(), 5);
}

#[test]
fn submit_with_zero_drafts_approve() {
    let e = env();
    let before = max_seq(&e.core);
    let sub = e
        .core
        .submit_review(&e.opened.review_id, Verdict::Approve, "", None)
        .unwrap();
    assert_eq!(sub.comment_count, 0);
    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["review.submitted"]);
    assert_eq!(evs[0].4["verdict"], "approve");
    assert_eq!(
        e.text("SELECT status FROM reviews").as_deref(),
        Some("approved")
    );
    assert_eq!(
        e.text("SELECT verdict FROM review_submissions").as_deref(),
        Some("approve")
    );
}

#[test]
fn submit_sets_review_status_per_verdict() {
    let e = env();
    for (verdict, status) in [
        (Verdict::RequestChanges, "changes_requested"),
        (Verdict::Comment, "commented"),
        (Verdict::Approve, "approved"),
    ] {
        e.core
            .submit_review(&e.opened.review_id, verdict, "", None)
            .unwrap();
        assert_eq!(
            e.text("SELECT status FROM reviews").as_deref(),
            Some(status)
        );
        assert_eq!(Verdict::parse(verdict.as_str()), Some(verdict));
    }
    assert_eq!(e.count("SELECT count(*) FROM review_submissions"), 3);
    assert!(matches!(
        e.core
            .submit_review("no-such-review", Verdict::Approve, "", None),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn submit_pins_live_state_first() {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    repo.checkout("feature");
    repo.write("a.txt", b"live\n");
    let core = core();
    let opened = core
        .open(&req(repo.path(), Source::Live { since: Since::Head }))
        .unwrap();
    assert!(opened.iteration.is_none());

    // Unpinned live review without an iteration: nothing to submit against.
    assert!(matches!(
        core.submit_review(&opened.review_id, Verdict::Comment, "", None),
        Err(CoreError::Conflict(_))
    ));

    // The composer pins before saving a draft (T3.10).
    let state = opened.live.clone().unwrap();
    let it1 = core
        .pin_live_on_base(
            &opened.review_id,
            &opened.base,
            &state,
            PinnedBy::Comment,
            &human_actor(),
        )
        .unwrap();
    let blobs = BlobReader::open(&opened.repo).unwrap();
    core.create_thread(
        &NewThread {
            review_id: opened.review_id.clone(),
            diff_id: it1.diff_id.clone(),
            subject: line("a.txt", Side::New, 1, 1),
            kind: ThreadKind::Comment,
            body_md: "hm".into(),
            author: human(),
        },
        &blobs,
    )
    .unwrap();

    repo.write("a.txt", b"live 2\n");
    let refreshed = core
        .open(&req(repo.path(), Source::Live { since: Since::Head }))
        .unwrap();
    assert_ne!(refreshed.diff_id, it1.diff_id);
    let before = max_seq(&core);

    let sub = core
        .submit_review(
            &opened.review_id,
            Verdict::Approve,
            "lgtm",
            Some((&refreshed.base, refreshed.live.as_ref().unwrap())),
        )
        .unwrap();

    assert_eq!(sub.iteration.seq, 2);
    assert_eq!(sub.iteration.diff_id, refreshed.diff_id);
    assert!(sub.iteration.snapshot_ref.is_some());
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM iterations WHERE seq = 2 AND pinned_by = 'submit'"
        ),
        1
    );
    assert_eq!(
        count(&core, "SELECT iteration_id FROM review_submissions"),
        sub.iteration.id
    );
    let evs = events_after(&core, before);
    assert_eq!(
        kinds(&evs),
        ["iteration.created", "thread.created", "review.submitted"]
    );
}

#[test]
fn agent_comment_never_draft() {
    let e = env();
    let before = max_seq(&e.core);
    let t = e.agent_thread(ThreadKind::Note, line("a.txt", Side::New, 5, 5), "changed");
    let r = e.core.reply(&t, "more", &agent()).unwrap();

    assert_eq!(
        e.count("SELECT count(*) FROM comments WHERE published_at IS NULL"),
        0
    );
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    assert!(!view.draft);
    assert_eq!(view.kind, ThreadKind::Note);
    assert_eq!(view.created_by.name, "claude-code");
    assert_eq!(view.comments[0].author.session_id.as_deref(), Some("s-1"));
    assert!(view.comments.iter().all(|c| !c.draft));
    assert_eq!(e.core.drafts_count(&e.opened.review_id).unwrap(), 0);

    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["thread.created", "comment.created"]);
    assert_eq!(evs[1].5.as_deref(), Some("claude-code"));
    assert_eq!(evs[0].2.as_deref(), Some(t.as_str()));
    assert_eq!(evs[0].4["kind"], "note");
    assert_eq!(evs[1].3.as_deref(), Some(r.as_str()));
    assert_eq!(evs[1].2.as_deref(), Some(t.as_str()));

    // A session the store does not know is not an error; it is left out.
    let unknown = Author {
        session_id: Some("s-unknown".into()),
        ..agent()
    };
    let r2 = e.core.reply(&t, "again", &unknown).unwrap();
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    let c = view.comments.iter().find(|c| c.id == r2).unwrap();
    assert_eq!(c.author.session_id, None);
}

#[test]
fn agent_thread_cap_50_per_iteration() {
    let e = env();
    for i in 0..50 {
        e.agent_thread(ThreadKind::Note, Subject::Review, &format!("n{i}"));
    }
    let over = e.core.create_thread(
        &new_thread(
            &e.opened,
            Subject::Review,
            ThreadKind::Question,
            "one more",
            agent_named("other-agent"),
        ),
        &e.blobs,
    );
    let err = over.unwrap_err();
    assert!(matches!(err, CoreError::CapExceeded { cap: 50 }), "{err:?}");
    assert_eq!(err.code(), "cap_exceeded");
    // Humans are not capped.
    e.human_thread(Subject::Review, "human");

    // A new iteration has its own budget.
    e.repo.checkout("feature");
    e.repo.write("a.txt", b"next\n");
    e.repo.commit("f2");
    e.repo.checkout("main");
    let next = e
        .core
        .open(&req(e.repo.path(), compare("main", "feature")))
        .unwrap();
    assert_eq!(next.iteration.as_ref().unwrap().seq, 2);
    e.core
        .create_thread(
            &new_thread(&next, Subject::Review, ThreadKind::Note, "new", agent()),
            &e.blobs,
        )
        .unwrap();
}

#[test]
fn agent_replies_do_not_count_toward_cap() {
    let e = env();
    let first = e.agent_thread(ThreadKind::Note, Subject::Review, "first");
    for i in 0..60 {
        e.core.reply(&first, &format!("r{i}"), &agent()).unwrap();
    }
    for i in 1..50 {
        e.agent_thread(ThreadKind::Note, Subject::Review, &format!("n{i}"));
    }
    let err = e
        .core
        .create_thread(
            &new_thread(&e.opened, Subject::Review, ThreadKind::Note, "x", agent()),
            &e.blobs,
        )
        .unwrap_err();
    assert!(matches!(err, CoreError::CapExceeded { .. }), "{err:?}");
    // Replies still work at the cap.
    e.core.reply(&first, "still fine", &agent()).unwrap();
}

#[test]
fn invalid_anchor_cases() {
    let e = env();
    let bad = |subject: Subject| {
        let err = e
            .core
            .create_thread(
                &new_thread(&e.opened, subject, ThreadKind::Note, "x", agent()),
                &e.blobs,
            )
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidAnchor(_)), "{err:?}");
        assert_eq!(err.code(), "invalid_anchor");
    };
    bad(line("nope.txt", Side::New, 1, 1)); // path not in diff
    bad(line("a.txt", Side::New, 11, 11)); // beyond the blob
    bad(line("a.txt", Side::New, 9, 11)); // range end beyond the blob
    bad(line("a.txt", Side::New, 5, 4)); // start_line > line
    bad(line("a.txt", Side::New, 0, 1)); // lines are 1-based
    bad(line("b.txt", Side::Old, 1, 1)); // old side of an added file
    bad(line("c.txt", Side::New, 1, 1)); // new side of a deleted file
    bad(line("d.bin", Side::New, 1, 1)); // binary files have no lines
    bad(Subject::File {
        path: "nope.txt".into(),
    });
    // Humans get the same checks.
    let err = e
        .core
        .create_thread(
            &new_thread(
                &e.opened,
                line("a.txt", Side::Old, 12, 12),
                ThreadKind::Comment,
                "x",
                human(),
            ),
            &e.blobs,
        )
        .unwrap_err();
    assert!(matches!(err, CoreError::InvalidAnchor(_)), "{err:?}");
    assert_eq!(e.count("SELECT count(*) FROM threads"), 0);

    // Valid edge cases.
    let ok = |subject: Subject| e.agent_thread(ThreadKind::Note, subject, "ok");
    ok(line("a.txt", Side::New, 10, 10)); // last line
    ok(line("a.txt", Side::Old, 1, 10)); // whole old side
    ok(line("c.txt", Side::Old, 1, 2)); // deleted file anchors old
    ok(line("b.txt", Side::New, 1, 2)); // added file anchors new
    ok(Subject::File {
        path: "d.bin".into(),
    });
    // A renamed file is found by its old path too and stored under its new path.
    let renamed = ok(line("old.txt", Side::New, 2, 3));
    let view = e.core.thread(&renamed, Viewer::Agent).unwrap();
    assert_eq!(view.anchor.subject, line("new.txt", Side::New, 2, 3));
    let deleted = ok(Subject::File {
        path: "c.txt".into(),
    });
    assert_eq!(
        e.core
            .thread(&deleted, Viewer::Agent)
            .unwrap()
            .anchor
            .subject,
        Subject::File {
            path: "c.txt".into()
        }
    );
}

#[test]
fn anchor_captures_blob_and_snippet_with_three_lines_of_context() {
    let e = env();
    let t = e.agent_thread(ThreadKind::Note, line("a.txt", Side::New, 5, 6), "x");
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    let new_blob = e.repo.oid("feature:a.txt");
    assert_eq!(view.anchor.anchor_blob.as_ref(), Some(&new_blob));
    assert_eq!(
        view.anchor.anchor_snippet.as_deref(),
        Some("l2\nl3\nl4\nL5\nl6\nl7\nl8\nl9")
    );

    let t = e.agent_thread(ThreadKind::Note, line("a.txt", Side::Old, 1, 1), "x");
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(view.anchor.anchor_blob, Some(e.repo.oid("main:a.txt")));
    assert_eq!(
        view.anchor.anchor_snippet.as_deref(),
        Some("l1\nl2\nl3\nl4")
    );

    let t = e.agent_thread(
        ThreadKind::Note,
        Subject::File {
            path: "a.txt".into(),
        },
        "x",
    );
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(view.anchor.anchor_blob, None);
    assert_eq!(view.anchor.anchor_snippet, None);
}

#[test]
fn note_and_question_require_agent_author() {
    let e = env();
    for kind in [ThreadKind::Note, ThreadKind::Question] {
        let err = e
            .core
            .create_thread(
                &new_thread(&e.opened, Subject::Review, kind, "x", human()),
                &e.blobs,
            )
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidRequest(_)), "{err:?}");
        assert_eq!(err.code(), "conflict");
    }
    // The schema enforces it too.
    let raw = e.core.store.write(|tx| {
        tx.execute(
            "INSERT INTO threads (id, review_id, origin_diff_id, subject, kind, created_by_kind, \
               created_by_name, created_at, updated_at) VALUES ('t', ?1, ?2, 'review', 'note', \
               'human', 'you', 1, 1)",
            [e.opened.review_id.as_str(), e.opened.diff_id.as_str()],
        )?;
        Ok(())
    });
    let msg = format!("{:?}", raw.unwrap_err());
    assert!(msg.contains("CHECK"), "{msg}");
    assert_eq!(e.count("SELECT count(*) FROM threads"), 0);
    // Agents may also leave plain comments.
    e.agent_thread(ThreadKind::Comment, Subject::Review, "fine");
}

#[test]
fn resolve_unresolve_records_actor_immediately() {
    let e = env();
    let t = e.agent_thread(ThreadKind::Question, Subject::Review, "q");
    let before = max_seq(&e.core);

    e.core.set_resolved(&t, true, &human_actor(), None).unwrap();
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(view.status, ThreadStatus::Resolved);
    let by = view.resolved_by.unwrap();
    assert_eq!(by.kind, AuthorKind::Human);
    assert_eq!(by.name.as_deref(), Some("you"));
    assert!(by.at > 0);
    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["thread.resolved"]);
    assert_eq!(evs[0].2.as_deref(), Some(t.as_str()));

    // Resolving again is a no-op.
    e.core.set_resolved(&t, true, &human_actor(), None).unwrap();
    assert_eq!(events_after(&e.core, before).len(), 1);

    let mid = max_seq(&e.core);
    e.core
        .set_resolved(&t, false, &agent_actor(), None)
        .unwrap();
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(view.status, ThreadStatus::Open);
    assert!(view.resolved_by.is_none());
    assert_eq!(kinds(&events_after(&e.core, mid)), ["thread.unresolved"]);

    // An agent's closing reply is published with the resolve.
    let mid = max_seq(&e.core);
    e.core
        .set_resolved(&t, true, &agent_actor(), Some("done"))
        .unwrap();
    let view = e.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(
        view.resolved_by.unwrap().name.as_deref(),
        Some("claude-code")
    );
    let last = view.comments.last().unwrap();
    assert_eq!(last.body_md, "done");
    assert!(!last.draft);
    assert_eq!(last.author.kind, AuthorKind::Agent);
    assert_eq!(
        kinds(&events_after(&e.core, mid)),
        ["comment.created", "thread.resolved"]
    );

    // A human's closing reply is a draft; the unresolve is still immediate.
    let mid = max_seq(&e.core);
    e.core
        .set_resolved(&t, false, &human_actor(), Some("reopening"))
        .unwrap();
    let view = e.core.thread(&t, Viewer::Human).unwrap();
    assert_eq!(view.status, ThreadStatus::Open);
    assert!(view.comments.last().unwrap().draft);
    assert_eq!(
        kinds(&events_after(&e.core, mid)),
        ["draft.changed", "thread.unresolved"]
    );

    // A draft thread cannot be resolved; the system actor cannot resolve.
    let draft = e.human_thread(Subject::Review, "draft");
    assert!(matches!(
        e.core.set_resolved(&draft, true, &human_actor(), None),
        Err(CoreError::Conflict(_))
    ));
    assert!(matches!(
        e.core.set_resolved(&t, true, &Actor::system(), None),
        Err(CoreError::InvalidRequest(_))
    ));
}

#[test]
fn edit_and_delete_only_own() {
    let e = env();
    let t = e.agent_thread(ThreadKind::Note, Subject::Review, "orig");
    let root = e.core.thread(&t, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();

    let before = max_seq(&e.core);
    e.core.edit_comment(&root, "edited", &agent()).unwrap();
    let c = &e.core.thread(&t, Viewer::Agent).unwrap().comments[0];
    assert_eq!(c.body_md, "edited");
    assert!(c.edited_at.is_some());
    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["comment.edited"]);
    assert_eq!(evs[0].3.as_deref(), Some(root.as_str()));

    // "Own" for an agent is the same author_name, whatever the session (OQ-30).
    let other_session = Author {
        session_id: Some("s-2".into()),
        ..agent()
    };
    e.core.edit_comment(&root, "again", &other_session).unwrap();
    for who in [agent_named("other-agent"), human()] {
        let err = e.core.edit_comment(&root, "x", &who).unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
        assert_eq!(err.code(), "forbidden");
        let err = e.core.delete_comment(&root, &who).unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
    }

    // Human comments: agents may not touch published ones and cannot see drafts.
    let reply = e.core.reply(&t, "draft", &human()).unwrap();
    let before = max_seq(&e.core);
    e.core.edit_comment(&reply, "draft 2", &human()).unwrap();
    let view = e.core.thread(&t, Viewer::Human).unwrap();
    let c = view.comments.iter().find(|c| c.id == reply).unwrap();
    assert_eq!(c.body_md, "draft 2");
    assert_eq!(c.edited_at, None, "editing a draft is not an edit");
    assert_eq!(kinds(&events_after(&e.core, before)), ["draft.changed"]);
    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();
    let err = e.core.edit_comment(&reply, "x", &agent()).unwrap_err();
    assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
    let err = e.core.delete_comment(&reply, &agent()).unwrap_err();
    assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
    let before = max_seq(&e.core);
    e.core
        .edit_comment(&reply, "published edit", &human())
        .unwrap();
    assert_eq!(kinds(&events_after(&e.core, before)), ["comment.edited"]);

    assert!(matches!(
        e.core.edit_comment("no-such-comment", "x", &human()),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn delete_published_with_replies_leaves_placeholder() {
    let e = env();
    let t = e.agent_thread(ThreadKind::Question, Subject::Review, "q?");
    let root = e.core.thread(&t, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();
    let reply = e.core.reply(&t, "answer", &human()).unwrap();
    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();

    let before = max_seq(&e.core);
    let out = e.core.delete_comment(&root, &agent()).unwrap();
    assert!(out.placeholder);
    assert!(!out.thread_removed);
    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["comment.deleted"]);
    assert_eq!(evs[0].2.as_deref(), Some(t.as_str()));

    for viewer in [Viewer::Agent, Viewer::Human] {
        let view = e.core.thread(&t, viewer).unwrap();
        assert_eq!(view.comments.len(), 2);
        assert!(view.comments[0].deleted);
        assert_eq!(view.comments[0].body_md, "");
        assert_eq!(view.comments[1].body_md, "answer");
    }
    // Deleting it again: it is gone.
    assert!(matches!(
        e.core.delete_comment(&root, &agent()),
        Err(CoreError::NotFound { .. })
    ));

    // Deleting the last live comment removes the thread.
    let out = e.core.delete_comment(&reply, &human()).unwrap();
    assert!(!out.placeholder);
    assert!(out.thread_removed);
    assert!(matches!(
        e.core.thread(&t, Viewer::Human),
        Err(CoreError::NotFound { .. })
    ));

    // A published reply without replies of its own leaves no placeholder.
    let t2 = e.agent_thread(ThreadKind::Note, Subject::Review, "n");
    let r2 = e.core.reply(&t2, "extra", &agent()).unwrap();
    let out = e.core.delete_comment(&r2, &agent()).unwrap();
    assert!(!out.placeholder);
    assert!(!out.thread_removed);
    assert_eq!(e.core.thread(&t2, Viewer::Agent).unwrap().comments.len(), 1);
}

#[test]
fn delete_draft_removes_it() {
    let e = env();
    let t = e.agent_thread(ThreadKind::Note, Subject::Review, "n");
    let reply = e.core.reply(&t, "draft", &human()).unwrap();
    let before = max_seq(&e.core);
    let out = e.core.delete_comment(&reply, &human()).unwrap();
    assert!(!out.placeholder);
    assert!(!out.thread_removed);
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM comments WHERE id = '{reply}'"
        )),
        0
    );
    // No agent-visible event for a draft.
    assert_eq!(kinds(&events_after(&e.core, before)), ["draft.changed"]);

    // Deleting a draft root removes the whole draft thread.
    let draft = e.human_thread(Subject::Review, "root");
    e.core.reply(&draft, "more", &human()).unwrap();
    let root = e.core.thread(&draft, Viewer::Human).unwrap().comments[0]
        .id
        .clone();
    let out = e.core.delete_comment(&root, &human()).unwrap();
    assert!(out.thread_removed);
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM threads WHERE id = '{draft}'"
        )),
        0
    );
    assert_eq!(e.core.drafts_count(&e.opened.review_id).unwrap(), 0);
}

#[test]
fn suggestion_blocks_parsed_structurally() {
    assert_eq!(
        parse_suggestions("Try this:\n\n```suggestion\nlet x = 1;\nlet y = 2;\n```\n"),
        ["let x = 1;\nlet y = 2;"]
    );
    // Empty suggestion = delete the anchored lines; tildes and info strings work.
    assert_eq!(
        parse_suggestions("```suggestion\n```\n\n~~~suggestion extra\nz\n~~~"),
        ["", "z"]
    );
    // Not suggestions: other languages, inline code, indented code, a suggestion
    // fence nested inside a longer fence, and a quoted one.
    let body = "```rust\nfn a() {}\n```\n\n`suggestion`\n\n    ```suggestion\n    no\n    ```\n\n\
                ````markdown\n```suggestion\ninner\n```\n````\n\n> ```suggestion\n> quoted\n> ```\n";
    assert!(
        parse_suggestions(body).is_empty(),
        "{:?}",
        parse_suggestions(body)
    );
    // Unclosed fences run to the end of the body (CommonMark).
    assert_eq!(parse_suggestions("```suggestion\nopen"), ["open"]);
    // Several blocks, in order.
    assert_eq!(
        parse_suggestions("```suggestion\na\n```\ntext\n```suggestion\nb\n```"),
        ["a", "b"]
    );
    assert_eq!(
        parse_suggestions("```Suggestion\nx\n```"),
        Vec::<String>::new()
    );
}

#[test]
fn rereview_sets_status_summary_and_event() {
    let e = env();
    let before = max_seq(&e.core);
    let it = e
        .core
        .request_rereview(&e.opened.review_id, "fixed it", &agent_actor(), None)
        .unwrap();
    assert_eq!(&it, e.opened.iteration.as_ref().unwrap());
    assert_eq!(
        e.text("SELECT status FROM reviews").as_deref(),
        Some("rereview_requested")
    );
    assert_eq!(
        e.text("SELECT rereview_summary FROM reviews").as_deref(),
        Some("fixed it")
    );
    assert!(e.count("SELECT rereview_at FROM reviews") > 0);
    let evs = events_after(&e.core, before);
    assert_eq!(kinds(&evs), ["review.rereview_requested"]);
    assert_eq!(evs[0].4["summary"], "fixed it");
    assert!(e.core.review_awaiting_you(&e.opened.review_id).unwrap());
    assert!(matches!(
        e.core
            .request_rereview("no-such-review", "", &agent_actor(), None),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn rereview_on_live_pins_the_worktree_as_a_new_iteration() {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    let core = core();
    let opened = core
        .open(&OpenRequest {
            pin: Some(PinnedBy::Agent),
            ..req(repo.path(), Source::Live { since: Since::Head })
        })
        .unwrap();
    assert_eq!(opened.iteration.as_ref().unwrap().seq, 1);
    repo.write("a.txt", b"changed by the agent\n");
    let now = core
        .open(&req(repo.path(), Source::Live { since: Since::Head }))
        .unwrap();
    let it = core
        .request_rereview(
            &opened.review_id,
            "",
            &agent_actor(),
            Some((&now.base, now.live.as_ref().unwrap())),
        )
        .unwrap();
    assert_eq!(it.seq, 2);
    assert_eq!(it.diff_id, now.diff_id);
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM iterations WHERE seq = 2 AND pinned_by = 'rereview'"
        ),
        1
    );
    assert_eq!(
        count(
            &core,
            "SELECT count(*) FROM reviews WHERE status = 'rereview_requested'"
        ),
        1
    );
}

#[test]
fn question_awaits_human_until_submitted_reply_or_resolve() {
    let e = env();
    let review = e.opened.review_id.as_str();
    assert!(!e.core.review_awaiting_you(review).unwrap());
    let note = e.agent_thread(ThreadKind::Note, Subject::Review, "fyi");
    assert!(!e.core.thread(&note, Viewer::Human).unwrap().awaiting_you());
    assert!(!e.core.review_awaiting_you(review).unwrap());

    let q = e.agent_thread(ThreadKind::Question, Subject::Review, "a or b?");
    assert!(e.core.thread(&q, Viewer::Human).unwrap().awaiting_you());
    assert!(e.core.review_awaiting_you(review).unwrap());

    // A draft reply does not answer it; the submitted one does.
    e.core.reply(&q, "a", &human()).unwrap();
    assert!(e.core.thread(&q, Viewer::Human).unwrap().awaiting_you());
    assert!(e.core.review_awaiting_you(review).unwrap());
    e.core
        .submit_review(review, Verdict::Comment, "", None)
        .unwrap();
    assert!(!e.core.thread(&q, Viewer::Human).unwrap().awaiting_you());
    assert!(!e.core.review_awaiting_you(review).unwrap());

    // Resolving answers it too; unresolving asks again.
    let q2 = e.agent_thread(ThreadKind::Question, Subject::Review, "c?");
    assert!(e.core.review_awaiting_you(review).unwrap());
    e.core
        .set_resolved(&q2, true, &agent_actor(), None)
        .unwrap();
    assert!(!e.core.thread(&q2, Viewer::Human).unwrap().awaiting_you());
    assert!(!e.core.review_awaiting_you(review).unwrap());
    e.core
        .set_resolved(&q2, false, &human_actor(), None)
        .unwrap();
    assert!(e.core.review_awaiting_you(review).unwrap());
}

#[test]
fn threads_from_other_clone_visible_via_origin_diff_id() {
    let e = env();
    let dir = tempfile::tempdir().unwrap();
    let clone = std::fs::canonicalize(dir.path()).unwrap().join("clone");
    e.repo.git(&[
        "clone",
        "-q",
        e.repo.path().to_str().unwrap(),
        clone.to_str().unwrap(),
    ]);
    let other = e
        .core
        .open(&req(&clone, compare("origin/main", "origin/feature")))
        .unwrap();
    assert_ne!(other.review_id, e.opened.review_id);
    assert_eq!(other.diff_id, e.opened.diff_id);

    let note = e.agent_thread(ThreadKind::Note, line("a.txt", Side::New, 5, 5), "n");
    let draft = e.human_thread(Subject::Review, "pending in the first review");
    let scope = ThreadScope::Review(other.review_id.clone());

    let ids = |viewer| -> Vec<String> {
        e.core
            .threads(scope.clone(), viewer, &all())
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect()
    };
    assert_eq!(ids(Viewer::Agent), vec![note.clone()]);
    // The first review's drafts are not this review's drafts.
    assert_eq!(ids(Viewer::Human), vec![note.clone()]);

    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();
    assert_eq!(ids(Viewer::Agent), [note.clone(), draft.clone()]);
    assert_eq!(ids(Viewer::Human), [note, draft]);
}

#[test]
fn thread_filters_select_by_status_author_kind_path_and_since() {
    let e = env();
    let note = e.agent_thread(ThreadKind::Note, line("a.txt", Side::New, 5, 5), "n");
    let q = e.agent_thread(
        ThreadKind::Question,
        Subject::File {
            path: "b.txt".into(),
        },
        "q",
    );
    let mine = e.human_thread(line("a.txt", Side::Old, 1, 1), "mine");
    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();
    e.core
        .set_resolved(&note, true, &human_actor(), None)
        .unwrap();
    let after_resolve = max_seq(&e.core);
    e.core.reply(&q, "later", &agent()).unwrap();

    let ids = |f: ThreadFilter| -> Vec<String> {
        e.core
            .threads(e.review_scope(), Viewer::Agent, &f)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect()
    };
    assert_eq!(ids(all()), [note.clone(), q.clone(), mine.clone()]);
    assert_eq!(
        ids(ThreadFilter {
            status: Some(ThreadStatus::Open),
            ..all()
        }),
        [q.clone(), mine.clone()]
    );
    assert_eq!(
        ids(ThreadFilter {
            status: Some(ThreadStatus::Resolved),
            ..all()
        }),
        vec![note.clone()]
    );
    assert_eq!(
        ids(ThreadFilter {
            author: Some(AuthorKind::Human),
            ..all()
        }),
        vec![mine.clone()]
    );
    assert_eq!(
        ids(ThreadFilter {
            kind: Some(ThreadKind::Question),
            ..all()
        }),
        vec![q.clone()]
    );
    assert_eq!(
        ids(ThreadFilter {
            path: Some("a.txt".into()),
            ..all()
        }),
        [note.clone(), mine]
    );
    assert_eq!(
        ids(ThreadFilter {
            since_seq: Some(after_resolve),
            ..all()
        }),
        [q]
    );
    assert!(matches!(
        e.core.threads(
            ThreadScope::Review("no-such-review".into()),
            Viewer::Agent,
            &all()
        ),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn create_thread_needs_a_known_review_and_stored_diff_and_a_body() {
    let e = env();
    let mut t = new_thread(&e.opened, Subject::Review, ThreadKind::Note, "x", agent());
    t.review_id = "no-such-review".into();
    assert!(matches!(
        e.core.create_thread(&t, &e.blobs),
        Err(CoreError::NotFound { what: "review", .. })
    ));
    let mut t = new_thread(&e.opened, Subject::Review, ThreadKind::Note, "x", agent());
    t.diff_id = DiffId::parse(&"ab".repeat(32)).unwrap();
    assert!(matches!(
        e.core.create_thread(&t, &e.blobs),
        Err(CoreError::NotFound { what: "diff", .. })
    ));
    let t = new_thread(&e.opened, Subject::Review, ThreadKind::Note, " \n", agent());
    assert!(matches!(
        e.core.create_thread(&t, &e.blobs),
        Err(CoreError::InvalidRequest(_))
    ));
    assert!(matches!(
        e.core.reply("no-such-thread", "x", &agent()),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn save_submit_draft_upserts_and_is_consumed_by_submit() {
    let e = env();
    let before = max_seq(&e.core);
    assert_eq!(e.core.submit_draft(&e.opened.review_id).unwrap(), None);
    e.core
        .save_submit_draft(&e.opened.review_id, "first", None)
        .unwrap();
    assert_eq!(
        e.core.submit_draft(&e.opened.review_id).unwrap(),
        Some(SubmitDraft {
            summary_md: "first".into(),
            verdict: None
        })
    );
    e.core
        .save_submit_draft(&e.opened.review_id, "second", Some(Verdict::Approve))
        .unwrap();
    assert_eq!(
        e.text("SELECT summary_md || ':' || verdict FROM review_drafts")
            .as_deref(),
        Some("second:approve")
    );
    assert_eq!(
        e.core.submit_draft(&e.opened.review_id).unwrap(),
        Some(SubmitDraft {
            summary_md: "second".into(),
            verdict: Some(Verdict::Approve)
        })
    );
    assert_eq!(
        kinds(&events_after(&e.core, before)),
        ["draft.changed", "draft.changed"]
    );
    assert!(matches!(
        e.core.save_submit_draft("no-such-review", "", None),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        e.core.submit_draft("no-such-review"),
        Err(CoreError::NotFound { .. })
    ));
    e.core
        .submit_review(&e.opened.review_id, Verdict::Approve, "second", None)
        .unwrap();
    assert_eq!(e.count("SELECT count(*) FROM review_drafts"), 0);
    assert_eq!(e.core.submit_draft(&e.opened.review_id).unwrap(), None);
}

#[test]
fn deleting_the_last_draft_under_a_deleted_root_removes_the_thread() {
    let e = env();
    let review = e.opened.review_id.as_str();
    let q = e.agent_thread(ThreadKind::Question, Subject::Review, "a or b?");
    let root = e.core.thread(&q, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();
    let draft = e.core.reply(&q, "a", &human()).unwrap();

    // The draft reply keeps the thread alive when the agent deletes its root.
    let out = e.core.delete_comment(&root, &agent()).unwrap();
    assert!(!out.thread_removed);
    assert!(!out.placeholder, "no published reply to show under it");
    let view = e.core.thread(&q, Viewer::Human).unwrap();
    assert_eq!(view.comments.len(), 2);
    assert!(view.comments[0].deleted);

    // Deleting that draft leaves no undeleted comment: the thread goes too.
    let before = max_seq(&e.core);
    let out = e.core.delete_comment(&draft, &human()).unwrap();
    assert!(out.thread_removed);
    assert!(!out.placeholder);
    assert_eq!(
        e.count(&format!("SELECT count(*) FROM threads WHERE id = '{q}'")),
        0
    );
    assert_eq!(kinds(&events_after(&e.core, before)), ["draft.changed"]);
    assert!(matches!(
        e.core.thread(&q, Viewer::Human),
        Err(CoreError::NotFound { .. })
    ));
    assert!(!e.core.review_awaiting_you(review).unwrap());
    assert_eq!(e.core.drafts_count(review).unwrap(), 0);
}

#[test]
fn agents_cannot_act_on_threads_they_cannot_see() {
    let e = env();
    let q = e.agent_thread(ThreadKind::Question, Subject::Review, "a or b?");
    let root = e.core.thread(&q, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();
    e.core.reply(&q, "a", &human()).unwrap();
    e.core.delete_comment(&root, &agent()).unwrap();

    // Only the human's draft is left: the thread does not exist for agents.
    assert!(matches!(
        e.core.thread(&q, Viewer::Agent),
        Err(CoreError::NotFound { .. })
    ));
    assert!(
        e.core
            .threads(e.review_scope(), Viewer::Agent, &all())
            .unwrap()
            .is_empty()
    );
    let before = max_seq(&e.core);
    assert!(matches!(
        e.core.reply(&q, "still there?", &agent()),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        e.core.set_resolved(&q, true, &agent_actor(), None),
        Err(CoreError::NotFound { .. })
    ));
    assert!(matches!(
        e.core.set_resolved(&q, true, &agent_actor(), Some("done")),
        Err(CoreError::NotFound { .. })
    ));
    assert_eq!(max_seq(&e.core), before, "nothing was written");

    // The human still sees it and may resolve it; once submitted, agents see it.
    e.core.set_resolved(&q, true, &human_actor(), None).unwrap();
    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();
    let view = e.core.thread(&q, Viewer::Agent).unwrap();
    assert_eq!(view.comments.len(), 2);
    assert!(view.comments[0].deleted);
    e.core.reply(&q, "thanks", &agent()).unwrap();
}

#[test]
fn human_author_names_are_stored_as_you() {
    let e = env();
    let bob = Author {
        kind: AuthorKind::Human,
        name: "bob".into(),
        session_id: Some("s-1".into()),
    };
    let t = e
        .core
        .create_thread(
            &new_thread(
                &e.opened,
                Subject::Review,
                ThreadKind::Comment,
                "hi",
                bob.clone(),
            ),
            &e.blobs,
        )
        .unwrap();
    e.core.reply(&t, "more", &bob).unwrap();
    let bob_actor = Actor {
        kind: ActorKind::Human,
        name: Some("bob".into()),
        session_id: Some("s-1".into()),
    };
    e.core
        .submit_review(&e.opened.review_id, Verdict::Comment, "", None)
        .unwrap();
    e.core
        .set_resolved(&t, true, &bob_actor, Some("bye"))
        .unwrap();

    let view = e.core.thread(&t, Viewer::Human).unwrap();
    assert_eq!(view.created_by.name, "you");
    assert_eq!(view.created_by.session_id, None);
    assert_eq!(view.resolved_by.unwrap().name.as_deref(), Some("you"));
    assert_eq!(view.comments.len(), 3);
    for c in &view.comments {
        assert_eq!(c.author.name, "you");
        assert_eq!(c.author.session_id, None);
    }
    assert_eq!(
        e.count(
            "SELECT count(*) FROM events WHERE actor_name IS NOT NULL AND actor_name <> 'you' \
             AND actor_kind = 'human'"
        ),
        0
    );
}

#[test]
fn submit_rolls_back_everything_when_a_step_fails() {
    let e = env();
    let review = e.opened.review_id.as_str();
    let t = e.human_thread(line("a.txt", Side::New, 5, 5), "one");
    e.core.reply(&t, "two", &human()).unwrap();
    e.core
        .save_submit_draft(review, "sum", Some(Verdict::Approve))
        .unwrap();
    let status = e.text("SELECT status FROM reviews");
    // Fail the very last write of the submission.
    e.core
        .store
        .write(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER fail_submit BEFORE INSERT ON events \
                 WHEN NEW.kind = 'review.submitted' \
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let before = max_seq(&e.core);

    let err = e
        .core
        .submit_review(review, Verdict::RequestChanges, "fix", None)
        .unwrap_err();
    assert!(format!("{err:?}").contains("injected failure"), "{err:?}");

    assert_eq!(max_seq(&e.core), before, "no event survives");
    assert_eq!(e.core.drafts_count(review).unwrap(), 2);
    assert_eq!(
        e.count("SELECT count(*) FROM comments WHERE published_at IS NOT NULL OR submission_id IS NOT NULL"),
        0
    );
    assert_eq!(e.count("SELECT count(*) FROM review_submissions"), 0);
    assert_eq!(e.count("SELECT count(*) FROM review_drafts"), 1);
    assert_eq!(e.text("SELECT status FROM reviews"), status);
    assert!(e.core.thread(&t, Viewer::Human).unwrap().draft);

    e.core
        .store
        .write(|tx| {
            tx.execute_batch("DROP TRIGGER fail_submit;")?;
            Ok(())
        })
        .unwrap();
    let sub = e
        .core
        .submit_review(review, Verdict::RequestChanges, "fix", None)
        .unwrap();
    assert_eq!(sub.comment_count, 2);
}

#[test]
fn submit_pins_the_displayed_merge_base_after_main_moves() {
    let _sb = Sandbox::isolate();
    let repo = review_repo();
    repo.checkout("feature");
    let f1 = repo.oid("HEAD");
    repo.write("b.txt", b"bee\nbuzz\nbzz\n");
    repo.commit("f2");
    repo.write("a.txt", b"live\n");
    let core = core();
    let live = || Source::Live {
        since: Since::MergeBase,
    };
    let opened = core.open(&req(repo.path(), live())).unwrap();
    assert_ne!(opened.base.commit.as_ref(), Some(&f1));

    // main moves after the tab rendered: the merge-base is now f1.
    repo.git(&["branch", "-f", "main", f1.as_str()]);
    let refreshed = core.open(&req(repo.path(), live())).unwrap();
    assert_eq!(refreshed.base.commit.as_ref(), Some(&f1));
    assert_ne!(refreshed.diff_id, opened.diff_id);

    // Submitting what the human saw pins the displayed base, not a re-resolved one.
    let sub = core
        .submit_review(
            &opened.review_id,
            Verdict::Approve,
            "lgtm",
            Some((&opened.base, opened.live.as_ref().unwrap())),
        )
        .unwrap();
    assert_eq!(sub.iteration.diff_id, opened.diff_id);
    assert_eq!(sub.iteration.seq, 1);
}

#[test]
fn review_activity_counts_agent_threads_after_last_seen() {
    let e = env();
    let review = e.opened.review_id.as_str();
    // Nothing yet.
    let a = e.core.review_activity(review).unwrap();
    assert_eq!(a.last_seen_seq, 0);
    assert!(a.unread.is_empty());
    assert!(a.rereview.is_none());

    // The human's own activity (drafts, a published thread) never counts.
    let mine = e.human_thread(line("a.txt", Side::New, 1, 1), "mine");
    e.core
        .submit_review(review, Verdict::Comment, "", None)
        .unwrap();
    assert!(e.core.review_activity(review).unwrap().unread.is_empty());

    // An agent note, an agent reply to the human's thread, and an agent
    // resolve: three threads, ordered by their latest event.
    let note = e.agent_thread(ThreadKind::Note, line("a.txt", Side::New, 5, 5), "a note");
    let question = e.agent_thread(ThreadKind::Question, Subject::Review, "why?");
    e.core.reply(&mine, "done", &agent()).unwrap();
    e.core
        .reply(&note, "and more", &agent_named("codex"))
        .unwrap();
    let a = e.core.review_activity(review).unwrap();
    let ids: Vec<&str> = a.unread.iter().map(|u| u.thread_id.as_str()).collect();
    assert_eq!(ids, [question.as_str(), mine.as_str(), note.as_str()]);
    assert!(a.unread.windows(2).all(|w| w[0].last_seq < w[1].last_seq));
    // The latest event's actor names the thread's agent.
    assert_eq!(a.unread[2].actor_name.as_deref(), Some("codex"));
    assert_eq!(a.unread[0].actor_name.as_deref(), Some("claude-code"));

    // Seen up to the reply on `mine`: only the note (replied to later)
    // stays unread.
    let seen = a.unread[1].last_seq;
    e.core.mark_seen(review, seen).unwrap();
    let a = e.core.review_activity(review).unwrap();
    assert_eq!(a.last_seen_seq, seen);
    assert_eq!(
        a.unread
            .iter()
            .map(|u| u.thread_id.as_str())
            .collect::<Vec<_>>(),
        [note.as_str()]
    );

    // A thread the agent deleted (its only comment) is not listed.
    let gone = e.agent_thread(ThreadKind::Note, Subject::Review, "oops");
    let root = e.core.thread(&gone, Viewer::Human).unwrap().comments[0]
        .id
        .clone();
    e.core.delete_comment(&root, &agent()).unwrap();
    let a = e.core.review_activity(review).unwrap();
    assert!(a.unread.iter().all(|u| u.thread_id != gone));

    // Re-review: shown while the status is `rereview_requested`, with the
    // summary and who asked; a submission ends it.
    e.core
        .request_rereview(review, "Fixed both.\n\nDetails.", &agent_actor(), None)
        .unwrap();
    let rereview = e.core.review_activity(review).unwrap().rereview.unwrap();
    assert_eq!(rereview.summary, "Fixed both.\n\nDetails.");
    assert_eq!(rereview.requested_by.as_deref(), Some("claude-code"));
    assert!(rereview.at > 0);
    e.core
        .submit_review(review, Verdict::Approve, "", None)
        .unwrap();
    assert!(e.core.review_activity(review).unwrap().rereview.is_none());

    assert!(matches!(
        e.core.review_activity("nope"),
        Err(CoreError::NotFound { .. })
    ));
}

#[test]
fn comment_lookup_follows_visibility() {
    let e = env();
    let agent_thread = e.agent_thread(ThreadKind::Note, Subject::Review, "note");
    let root = e
        .core
        .thread(&agent_thread, Viewer::Agent)
        .unwrap()
        .comments[0]
        .id
        .clone();
    let reply = e.core.reply(&agent_thread, "more", &agent()).unwrap();
    e.core.edit_comment(&reply, "edited", &agent()).unwrap();

    let seen = e.core.comment(&reply, Viewer::Agent).unwrap();
    assert_eq!(seen.thread_id, agent_thread);
    assert_eq!(seen.body_md, "edited");
    assert!(seen.edited_at.is_some());
    assert_eq!(
        e.core.comment(&root, Viewer::Human).unwrap().body_md,
        "note"
    );

    // A human draft exists for the human only.
    let draft_thread = e.human_thread(Subject::Review, "draft");
    let draft = e
        .core
        .thread(&draft_thread, Viewer::Human)
        .unwrap()
        .comments[0]
        .id
        .clone();
    assert!(e.core.comment(&draft, Viewer::Human).unwrap().draft);
    assert_eq!(
        e.core.comment(&draft, Viewer::Agent).unwrap_err().code(),
        "not_found"
    );
    assert_eq!(
        e.core.comment("nope", Viewer::Human).unwrap_err().code(),
        "not_found"
    );

    // A deleted root with replies stays as a placeholder; a deleted reply is gone.
    e.core.delete_comment(&reply, &agent()).unwrap();
    assert_eq!(
        e.core.comment(&reply, Viewer::Agent).unwrap_err().code(),
        "not_found"
    );
}
