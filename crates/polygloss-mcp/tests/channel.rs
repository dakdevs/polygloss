//! Opt-in `claude/channel` push (T4.13, design §16.4, OQ-33): the capability,
//! the notification shape and the submission watch behind it. The bun suite
//! `tests/mcp/channel.test.ts` drives the real server end to end.

use polygloss_core::ObjectFormat;
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AssignedBy, Author, AuthorKind, Core, NewThread, OpenRequest, SessionInfo, Subject, ThreadKind,
    Verdict,
};
use polygloss_core::store::events::Actor;
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_mcp::ServeOptions;
use polygloss_mcp::channel::{self, CHANNEL_CAPABILITY, CHANNEL_METHOD, SubmissionWatch};
use polygloss_mcp::wake::Wake;
use rmcp::model::{ServerCapabilities, ServerNotification};
use serde_json::json;

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

fn caps() -> ServerCapabilities {
    ServerCapabilities::builder()
        .enable_tools()
        .enable_resources()
        .build()
}

#[test]
fn declare_adds_the_capability_only_with_the_option() {
    let mut off = caps();
    channel::declare(&ServeOptions { channel: false }, &mut off);
    assert_eq!(off, caps());
    assert!(off.experimental.is_none());

    let mut on = caps();
    channel::declare(&ServeOptions { channel: true }, &mut on);
    let v = serde_json::to_value(&on).unwrap();
    assert_eq!(v["experimental"][CHANNEL_CAPABILITY], json!({}));
    assert_eq!(CHANNEL_CAPABILITY, "claude/channel");
    // Tools and resources stay declared.
    assert!(v["tools"].is_object() && v["resources"].is_object(), "{v}");
}

#[test]
fn notification_carries_the_wake_text_and_string_meta() {
    let wake = Wake {
        review_id: "0199-review".into(),
        submission_id: "0199-sub".into(),
        verdict: Verdict::RequestChanges,
        seq: 7,
        text: "Polygloss: the human submitted …".into(),
    };
    let n = channel::notification(&wake);
    let ServerNotification::CustomNotification(custom) = &n else {
        panic!("not a custom notification: {n:?}");
    };
    assert_eq!(custom.method, CHANNEL_METHOD);
    assert_eq!(CHANNEL_METHOD, "notifications/claude/channel");
    assert_eq!(
        custom.params,
        Some(json!({
            "content": "Polygloss: the human submitted …",
            "meta": {
                "review_id": "0199-review",
                "submission_id": "0199-sub",
                "verdict": "request_changes",
            },
        }))
    );
    // On the wire it is a plain JSON-RPC notification.
    let wire = serde_json::to_value(&n).unwrap();
    assert_eq!(wire["method"], CHANNEL_METHOD);
    assert_eq!(wire["params"]["meta"]["verdict"], "request_changes");
}

#[test]
fn watch_reports_new_submissions_of_the_sessions_reviews_once() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let (r1, r2) = (repo(), repo());
    let mine = open_review(&core, &r1, "Mine");
    let theirs = open_review(&core, &r2, "Theirs");
    session(&core, "me", None);
    session(&core, "other", None);
    core.assign_review(&mine, "me", AssignedBy::OpenDiff)
        .unwrap();
    core.assign_review(&theirs, "other", AssignedBy::OpenDiff)
        .unwrap();

    // A submission from before the watch opened is not pushed (the Stop hook
    // or `wait_for_review` report it).
    core.submit_review(&mine, Verdict::Comment, "Old.", None)
        .unwrap();
    let mut watch = SubmissionWatch::open(&core, "me").unwrap();
    assert!(watch.poll().unwrap().is_empty());

    core.submit_review(&theirs, Verdict::Approve, "", None)
        .unwrap();
    assert!(watch.poll().unwrap().is_empty(), "another session's review");

    let s = core
        .submit_review(&mine, Verdict::RequestChanges, "Fix it.", None)
        .unwrap();
    let wakes = watch.poll().unwrap();
    assert_eq!(wakes.len(), 1);
    assert_eq!(wakes[0].review_id, mine);
    assert_eq!(wakes[0].submission_id, s.id);
    assert_eq!(wakes[0].seq, s.seq);
    assert!(
        wakes[0].text.contains("Summary: Fix it."),
        "{}",
        wakes[0].text
    );
    // Each submission once.
    assert!(watch.poll().unwrap().is_empty());
}

#[test]
fn watch_follows_the_canonical_session_and_ignores_agent_replies() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let r = repo();
    let review = open_review(&core, &r, "Linked");
    // The MCP server keeps its spawn id; after `/clear` the hook's id differs
    // but links to it through the owner pid (§16.4).
    session(&core, "spawned", Some(900_101));
    session(&core, "after-clear", Some(900_101));
    core.assign_review(&review, "after-clear", AssignedBy::OpenDiff)
        .unwrap();
    let mut watch = SubmissionWatch::open(&core, "spawned").unwrap();

    // Whatever the agent writes, only a submission wakes (OQ-11).
    let latest = core.iterations(&review).unwrap().pop().unwrap();
    let (repo_path, _) = core
        .find_repo_for_diff(latest.diff_id.as_str(), None)
        .unwrap();
    let blobs = BlobReader::open(&repo_path).unwrap();
    let agent = Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: Some("spawned".into()),
    };
    let thread_id = core
        .create_thread(
            &NewThread {
                review_id: review.clone(),
                diff_id: latest.diff_id.clone(),
                subject: Subject::Review,
                kind: ThreadKind::Note,
                body_md: "Explaining the change.".into(),
                author: agent.clone(),
            },
            &blobs,
        )
        .unwrap();
    core.reply(&thread_id, "And one more detail.", &agent)
        .unwrap();
    assert!(watch.poll().unwrap().is_empty());

    let s = core
        .submit_review(&review, Verdict::Approve, "", None)
        .unwrap();
    let wakes = watch.poll().unwrap();
    assert_eq!(wakes.len(), 1);
    assert_eq!(wakes[0].submission_id, s.id);
    assert_eq!(wakes[0].verdict, Verdict::Approve);
}
