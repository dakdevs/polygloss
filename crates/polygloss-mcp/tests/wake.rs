//! Wake-ups for `polygloss wait` (T4.8, design §16.3) and the opt-in channel
//! push (T4.13): which `review.submitted` events concern a session, and the
//! summary text an agent is woken with.

use polygloss_core::ObjectFormat;
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{AssignedBy, Core, OpenRequest, SessionInfo, Verdict};
use polygloss_core::store::events::{Actor, Event, EventFilter, EventKind, events_since};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_mcp::wake::{self, Wake};

fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\ntwo\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", b"one\n2\n");
    repo.commit("f1");
    repo.checkout("main");
    repo
}

fn open_review(core: &Core, repo: &FixtureRepo, label: &str) -> String {
    core.open(&OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "main".into(),
            head: "feature".into(),
            mode: CompareMode::ThreeDot,
        },
        label: Some(label.into()),
        pin: None,
        actor: Actor::human(),
    })
    .unwrap()
    .review_id
}

fn session(core: &Core, id: &str, pid: Option<i32>) {
    core.upsert_session(&SessionInfo {
        id: id.into(),
        client_name: "claude-code".into(),
        client_version: None,
        owner_pid: pid,
        cwd: None,
    })
    .unwrap();
}

fn submitted_since(core: &Core, after: i64) -> Vec<Event> {
    core.store
        .read(|c| {
            events_since(
                c,
                after,
                &EventFilter {
                    kinds: Some(vec![EventKind::ReviewSubmitted]),
                    ..EventFilter::default()
                },
                1000,
            )
        })
        .unwrap()
}

fn rereview(core: &Core, review_id: &str) {
    core.store
        .write(|tx| {
            tx.execute(
                "INSERT INTO events (at, kind, review_id, actor_kind, actor_name, payload) \
                 VALUES (0, 'review.rereview_requested', ?1, 'agent', 'claude-code', '{}')",
                [review_id],
            )?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn wakes_only_for_submissions_on_reviews_assigned_to_the_session() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let (r1, r2) = (repo(), repo());
    let mine = open_review(&core, &r1, "Mine");
    let theirs = open_review(&core, &r2, "Theirs");
    session(&core, "me", Some(900_001));
    session(&core, "me-after-clear", Some(900_001));
    session(&core, "other", None);
    core.assign_review(&mine, "me", AssignedBy::OpenDiff)
        .unwrap();
    core.assign_review(&theirs, "other", AssignedBy::OpenDiff)
        .unwrap();

    let a = core
        .submit_review(&mine, Verdict::RequestChanges, "Fix it.", None)
        .unwrap();
    core.submit_review(&theirs, Verdict::Approve, "", None)
        .unwrap();

    let events = submitted_since(&core, 0);
    assert_eq!(events.len(), 2);
    // The drifted id sees what its canonical session sees.
    for id in ["me", "me-after-clear"] {
        let wakes = wake::wakes_for(&core, id, &events).unwrap();
        assert_eq!(wakes.len(), 1, "{id}");
        let Wake {
            review_id,
            submission_id,
            verdict,
            seq,
            text,
        } = &wakes[0];
        assert_eq!(review_id, &mine);
        assert_eq!(submission_id, &a.id);
        assert_eq!(*verdict, Verdict::RequestChanges);
        assert_eq!(*seq, a.seq);
        assert!(text.contains("Mine"), "{text}");
        assert!(text.contains("Verdict: request changes"), "{text}");
        assert!(text.contains("Fix it."), "{text}");
        assert!(
            text.contains(&format!("list_threads(review_id=\"{mine}\")")),
            "{text}"
        );
    }
    let other = wake::wakes_for(&core, "other", &events).unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].verdict, Verdict::Approve);
    assert!(
        wake::wakes_for(&core, "stranger", &events)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn wake_reports_the_latest_submission_and_skips_answered_ones() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let r = repo();
    let review = open_review(&core, &r, "Twice");
    session(&core, "me", None);
    core.assign_review(&review, "me", AssignedBy::OpenDiff)
        .unwrap();

    core.submit_review(&review, Verdict::RequestChanges, "First.", None)
        .unwrap();
    let second = core
        .submit_review(&review, Verdict::Comment, "Second.", None)
        .unwrap();
    let wakes = wake::wakes_for(&core, "me", &submitted_since(&core, 0)).unwrap();
    assert_eq!(wakes.len(), 1);
    assert_eq!(wakes[0].submission_id, second.id);
    assert!(wakes[0].text.contains("Second."));
    assert!(!wakes[0].text.contains("First."));

    // The agent already answered with request_rereview: nothing to wake for.
    rereview(&core, &review);
    assert!(
        wake::wakes_for(&core, "me", &submitted_since(&core, 0))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn combined_text_joins_every_wake() {
    let wakes = vec![
        Wake {
            review_id: "r1".into(),
            submission_id: "s1".into(),
            verdict: Verdict::Approve,
            seq: 3,
            text: "first".into(),
        },
        Wake {
            review_id: "r2".into(),
            submission_id: "s2".into(),
            verdict: Verdict::Comment,
            seq: 4,
            text: "second".into(),
        },
    ];
    assert_eq!(wake::combined_text(&wakes), "first\n\nsecond");
}
