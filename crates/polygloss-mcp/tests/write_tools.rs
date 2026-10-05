//! The write tools (T4.6, design §15.2) called through `polygloss_mcp::api`
//! against a sandboxed store, with a recording launcher and a fake app bound
//! at the sandbox's socket: side effects the bun suite
//! (`tests/mcp/write-tools.test.ts`) checks end to end, plus the stored rows,
//! events and error codes behind them.
//!
//! Every test runs under `Sandbox::isolate()`; only nextest (one process per
//! test) is supported.

use std::sync::{Arc, Mutex};

use polygloss_core::ObjectFormat;
use polygloss_core::ipc::{IpcError, Op, ServerConfig, ServerHandle, serve_at, socket_path};
use polygloss_core::review::{
    Author, AuthorKind, Core, SessionInfo, Subject, ThreadKind, ThreadStatus, Verdict, Viewer,
};
use polygloss_core::store::events::{EventFilter, events_since};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_diff::Side;
use polygloss_mcp::api::shapes::{
    AgentThreadKind, AnchorParam, CompareModeParam, SideParam, SourceParam,
};
use polygloss_mcp::api::{
    self, CreateCommentRequest, DeleteCommentRequest, EditCommentRequest, FocusRequest,
    OpenDiffRequest, ReplyRequest, RequestRereviewRequest, ResolveRequest, UnresolveRequest,
};
use polygloss_mcp::{ApiContext, ApiErrorCode};
use polygloss_platform::launch::Launcher;
use serde_json::json;

const TEN: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
const TEN_EDITED: &str = "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\nl10\n";

type Ops = Arc<Mutex<Vec<Op>>>;

/// A fake app at the sandbox's socket that records every op and answers
/// `{status}`; `fail_focus` answers `focus` with that error code.
fn fake_app(core: &Core, ops: Ops, fail_focus: Option<&'static str>) -> ServerHandle {
    let path = socket_path(&core.paths);
    serve_at(&path, ServerConfig::from_lookup(|_| None), move |op: Op| {
        ops.lock().unwrap().push(op.clone());
        match (&op, fail_focus) {
            (Op::Focus { .. }, Some(code)) => Err(IpcError::new(code, "no such path")),
            _ => Ok(json!({ "status": "ok" })),
        }
    })
    .expect("bind the fake app")
}

/// Records launches; with `start_app`, a launch binds a fake app.
#[derive(Default)]
struct RecordingLauncher {
    calls: Mutex<Vec<(Option<String>, bool)>>,
    start_app: Option<(Core, Ops)>,
    started: Mutex<Option<ServerHandle>>,
    fail: bool,
}

impl Launcher for RecordingLauncher {
    fn launch(&self, url: Option<&str>, activate: bool) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((url.map(str::to_owned), activate));
        if self.fail {
            return Err(std::io::Error::other("no app in this test"));
        }
        if let Some((core, ops)) = &self.start_app {
            *self.started.lock().unwrap() = Some(fake_app(core, ops.clone(), None));
        }
        Ok(())
    }
}

impl RecordingLauncher {
    fn calls(&self) -> Vec<(Option<String>, bool)> {
        self.calls.lock().unwrap().clone()
    }
}

struct World {
    _sb: Sandbox,
    repo: FixtureRepo,
    core: Core,
}

/// `main` with `a.txt` (ten lines) and `b.txt`; branch `feature` changes line 5
/// of `a.txt`. The worktree is on `main`.
fn world() -> World {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", TEN.as_bytes());
    repo.write("b.txt", b"b1\nb2\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", TEN_EDITED.as_bytes());
    repo.commit("f1");
    repo.checkout("main");
    let core = Core::open_default().unwrap();
    record_session(&core, "sess-me", "claude-code");
    World {
        _sb: sb,
        repo,
        core,
    }
}

fn record_session(core: &Core, id: &str, client: &str) {
    core.upsert_session(&SessionInfo {
        id: id.into(),
        client_name: client.into(),
        client_version: None,
        owner_pid: None,
        cwd: None,
    })
    .unwrap();
}

impl World {
    fn ctx(&self, launcher: Arc<RecordingLauncher>) -> ApiContext {
        self.ctx_named("claude-code", "sess-me", launcher)
    }

    fn ctx_named(
        &self,
        client: &str,
        session: &str,
        launcher: Arc<RecordingLauncher>,
    ) -> ApiContext {
        ApiContext {
            core: self.core.clone(),
            session_id: session.into(),
            client_name: client.into(),
            launcher,
            roots: vec![self.repo.path().to_path_buf()],
        }
    }

    fn repo_arg(&self) -> Option<String> {
        Some(self.repo.path().to_string_lossy().into_owned())
    }

    /// A live review (since HEAD) of the worktree with line 5 edited, opened by
    /// the agent without showing it.
    fn live_review(&self, ctx: &ApiContext) -> api::OpenDiffResult {
        self.repo.write("a.txt", TEN_EDITED.as_bytes());
        api::open_diff(
            ctx,
            OpenDiffRequest {
                repo: self.repo_arg(),
                source: Some(SourceParam::Live {
                    since: Some("HEAD".into()),
                }),
                show: Some(false),
                ..OpenDiffRequest::default()
            },
        )
        .unwrap()
    }

    /// The first column of `sql`'s first row as text.
    fn scalar(&self, sql: &str) -> String {
        let wrapped = format!("SELECT CAST(({sql}) AS TEXT)");
        self.core
            .store
            .read(|c| Ok(c.query_row(&wrapped, [], |r| r.get::<_, String>(0))?))
            .unwrap()
    }

    fn event_kinds(&self) -> Vec<String> {
        self.core
            .store
            .read(|c| events_since(c, 0, &EventFilter::default(), 10_000))
            .unwrap()
            .into_iter()
            .map(|e| e.kind.as_str().to_owned())
            .collect()
    }
}

fn no_launch() -> Arc<RecordingLauncher> {
    Arc::new(RecordingLauncher {
        fail: true,
        ..RecordingLauncher::default()
    })
}

fn note(review_id: &str, body: &str, anchor: Option<AnchorParam>) -> CreateCommentRequest {
    CreateCommentRequest {
        review_id: Some(review_id.into()),
        diff_id: None,
        kind: AgentThreadKind::Note,
        body_md: body.into(),
        anchor,
    }
}

fn line_anchor(path: &str, line: u32) -> AnchorParam {
    AnchorParam {
        path: path.into(),
        side: Some(SideParam::New),
        line: Some(line),
        start_line: None,
    }
}

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

// ---- open_diff ----

#[test]
fn open_diff_live_pins_assigns_and_counts() {
    let w = world();
    let launcher = no_launch();
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);

    assert_eq!(r.iteration, 1);
    assert_eq!(r.url, format!("polygloss://diff/{}", r.diff_id));
    assert_eq!(r.app, polygloss_mcp::app_link::AppShown::Skipped);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["app"], "skipped");
    assert_eq!(
        v["stats"],
        json!({ "files": 1, "additions": 1, "deletions": 1 })
    );
    assert_eq!(
        v["files"],
        json!([{ "path": "a.txt", "status": "modified", "additions": 1, "deletions": 1 }])
    );
    assert_eq!(v["files_truncated"], false);
    assert!(v["base"]["commit"].is_string() && v["base"]["tree"].is_string());
    assert!(
        v["head"].get("commit").is_none(),
        "live heads are the worktree"
    );
    assert!(v.get("warnings").is_none());

    assert_eq!(w.scalar("SELECT pinned_by FROM iterations"), "agent");
    assert_eq!(
        w.scalar("SELECT session_id || ':' || assigned_by FROM review_assignments"),
        "sess-me:open_diff"
    );
    assert!(launcher.calls().is_empty(), "show=false never launches");

    // Unchanged worktree: the same iteration.
    let again = w.live_review(&ctx);
    assert_eq!(again.review_id, r.review_id);
    assert_eq!(again.iteration, 1);
}

#[test]
fn open_diff_latest_opener_wins_and_assign_false_keeps_it() {
    let w = world();
    record_session(&w.core, "sess-other", "claude-code");
    let mine = w.ctx(no_launch());
    let other = w.ctx_named("claude-code", "sess-other", no_launch());
    let r = w.live_review(&mine);
    w.live_review(&other);
    let assigned = || w.scalar("SELECT session_id FROM review_assignments");
    assert_eq!(assigned(), "sess-other");
    api::open_diff(
        &mine,
        OpenDiffRequest {
            repo: w.repo_arg(),
            source: Some(SourceParam::Live {
                since: Some("HEAD".into()),
            }),
            show: Some(false),
            assign: Some(false),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap();
    assert_eq!(assigned(), "sess-other");
    w.live_review(&mine);
    assert_eq!(assigned(), "sess-me");
    assert_eq!(
        w.scalar("SELECT count(*) FROM reviews"),
        "1",
        "{}",
        r.review_id
    );
}

#[test]
fn open_diff_compare_reports_refs_and_stores_the_label() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = api::open_diff(
        &ctx,
        OpenDiffRequest {
            repo: w.repo_arg(),
            source: Some(SourceParam::Compare {
                base: "main".into(),
                head: "feature".into(),
                mode: Some(CompareModeParam::Direct),
            }),
            label: Some("  PR #7 ".into()),
            show: Some(false),
            assign: None,
        },
    )
    .unwrap();
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["base"]["rev"], "refs/heads/main");
    assert_eq!(v["head"]["rev"], "refs/heads/feature");
    assert_eq!(v["head"]["commit"], w.repo.oid("feature").as_str());
    assert_eq!(w.scalar("SELECT label FROM reviews"), "PR #7");
    assert_eq!(w.scalar("SELECT pinned_by FROM iterations"), "open");
    assert!(r.review_key.starts_with("compare:"), "{}", r.review_key);
}

#[test]
fn open_diff_outside_git_is_repo_not_found() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let dir = tempfile::tempdir().unwrap();
    let err = api::open_diff(
        &ctx,
        OpenDiffRequest {
            repo: Some(dir.path().to_string_lossy().into_owned()),
            show: Some(false),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::RepoNotFound, "{err}");
}

#[test]
fn open_diff_show_opens_a_running_app_in_the_background() {
    let w = world();
    let ops: Ops = Arc::default();
    let _app = fake_app(&w.core, ops.clone(), None);
    let launcher = no_launch();
    let ctx = w.ctx(launcher.clone());
    w.repo.write("a.txt", TEN_EDITED.as_bytes());
    let r = api::open_diff(
        &ctx,
        OpenDiffRequest {
            repo: w.repo_arg(),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap();
    assert_eq!(r.app, polygloss_mcp::app_link::AppShown::Opened);
    assert!(launcher.calls().is_empty());
    let ops = ops.lock().unwrap().clone();
    assert!(matches!(ops[0], Op::StoreChanged { .. }), "{ops:?}");
    assert_eq!(
        ops[1],
        Op::Open {
            review_id: Some(r.review_id.clone()),
            diff_id: None,
            activate: false
        }
    );
    assert_eq!(ops.len(), 2);
}

#[test]
fn open_diff_show_launches_hidden_or_reports_unavailable() {
    let w = world();
    let ops: Ops = Arc::default();
    let launcher = Arc::new(RecordingLauncher {
        start_app: Some((w.core.clone(), ops.clone())),
        ..RecordingLauncher::default()
    });
    let ctx = w.ctx(launcher.clone());
    w.repo.write("a.txt", TEN_EDITED.as_bytes());
    let r = api::open_diff(
        &ctx,
        OpenDiffRequest {
            repo: w.repo_arg(),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap();
    assert_eq!(r.app, polygloss_mcp::app_link::AppShown::Launched);
    assert_eq!(launcher.calls(), [(None, false)]);
    assert!(ops.lock().unwrap().iter().any(|op| matches!(
        op,
        Op::Open { activate: false, review_id: Some(id), .. } if *id == r.review_id
    )));
    drop(launcher.started.lock().unwrap().take());

    // A launch that fails is an outcome, not an error.
    let ctx = w.ctx(no_launch());
    let r = api::open_diff(
        &ctx,
        OpenDiffRequest {
            repo: w.repo_arg(),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(r.app).unwrap(), "unavailable");
}

// ---- create_comment ----

#[test]
fn create_comment_on_live_pins_the_current_worktree() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    let c = api::create_comment(
        &ctx,
        note(&r.review_id, "why", Some(line_anchor("a.txt", 5))),
    )
    .unwrap();
    assert_eq!((c.iteration, c.diff_id.as_str()), (1, r.diff_id.as_str()));

    // The agent edits more and comments: a new agent-pinned iteration.
    w.repo.write("b.txt", b"b1\nB2\n");
    let c2 = api::create_comment(
        &ctx,
        note(&r.review_id, "and b", Some(line_anchor("b.txt", 2))),
    )
    .unwrap();
    assert_eq!(c2.iteration, 2);
    assert_ne!(c2.diff_id, r.diff_id);
    assert_eq!(
        w.scalar("SELECT group_concat(pinned_by) FROM iterations"),
        "agent,agent"
    );
    let t = w.core.thread(&c2.thread_id, Viewer::Agent).unwrap();
    assert_eq!(t.kind, ThreadKind::Note);
    assert!(!t.draft);
    assert_eq!(t.created_by.name, "claude-code");
    assert!(w.event_kinds().iter().any(|k| k == "thread.created"));
}

#[test]
fn create_comment_by_diff_prefix_and_its_errors() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    let q = api::create_comment(
        &ctx,
        CreateCommentRequest {
            review_id: None,
            diff_id: Some(r.diff_id[..10].to_owned()),
            kind: AgentThreadKind::Question,
            body_md: "ok?".into(),
            anchor: Some(AnchorParam {
                path: "a.txt".into(),
                side: None,
                line: None,
                start_line: None,
            }),
        },
    )
    .unwrap();
    let t = w.core.thread(&q.thread_id, Viewer::Agent).unwrap();
    assert_eq!(t.kind, ThreadKind::Question);
    assert_eq!(
        t.anchor.subject,
        Subject::File {
            path: "a.txt".into()
        }
    );
    assert_eq!(t.review_id.as_deref(), Some(r.review_id.as_str()));

    let code = |req: CreateCommentRequest| api::create_comment(&ctx, req).unwrap_err().code;
    assert_eq!(
        code(CreateCommentRequest {
            review_id: None,
            diff_id: None,
            ..note("x", "b", None)
        }),
        ApiErrorCode::Conflict
    );
    assert_eq!(
        code(note("no-such-review", "b", None)),
        ApiErrorCode::NotFound
    );
    assert_eq!(
        code(CreateCommentRequest {
            diff_id: Some("0123456789abcdef".into()),
            ..note(&r.review_id, "b", None)
        }),
        ApiErrorCode::NotFound
    );
    assert_eq!(code(note(&r.review_id, "  ", None)), ApiErrorCode::Conflict);
    for anchor in [
        line_anchor("nope.txt", 1),
        line_anchor("a.txt", 11),
        AnchorParam {
            start_line: Some(6),
            ..line_anchor("a.txt", 5)
        },
        AnchorParam {
            side: Some(SideParam::Old),
            line: None,
            ..line_anchor("a.txt", 5)
        },
    ] {
        assert_eq!(
            code(note(&r.review_id, "b", Some(anchor.clone()))),
            ApiErrorCode::InvalidAnchor,
            "{anchor:?}"
        );
    }
}

#[test]
fn create_comment_cap_is_fifty_per_iteration() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    for i in 0..50 {
        api::create_comment(&ctx, note(&r.review_id, &format!("n{i}"), None)).unwrap();
    }
    let err = api::create_comment(&ctx, note(&r.review_id, "one too many", None)).unwrap_err();
    assert_eq!(err.code, ApiErrorCode::CapExceeded, "{err}");
}

// ---- reply, resolve, unresolve ----

#[test]
fn reply_resolve_and_unresolve_record_the_client_name() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    let t = api::create_comment(&ctx, note(&r.review_id, "n", None))
        .unwrap()
        .thread_id;

    let reply = api::reply(
        &ctx,
        ReplyRequest {
            thread_id: t.clone(),
            body_md: "done".into(),
            resolve: None,
        },
    )
    .unwrap();
    assert_eq!(reply.status, ThreadStatus::Open);
    let resolved = api::reply(
        &ctx,
        ReplyRequest {
            thread_id: t.clone(),
            body_md: "fixed".into(),
            resolve: Some(true),
        },
    )
    .unwrap();
    assert_eq!(resolved.status, ThreadStatus::Resolved);
    let view = w.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(view.comments.len(), 3);
    assert!(view.comments.iter().all(|c| c.author.name == "claude-code"));
    let by = view.resolved_by.unwrap();
    assert_eq!(
        (by.kind, by.name.as_deref()),
        (AuthorKind::Agent, Some("claude-code"))
    );

    let open = api::unresolve(
        &ctx,
        UnresolveRequest {
            thread_id: t.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&open).unwrap(),
        json!({ "thread_id": t, "status": "open" })
    );

    let closed = api::resolve(
        &ctx,
        ResolveRequest {
            thread_id: t.clone(),
            body_md: Some("closing".into()),
        },
    )
    .unwrap();
    let v = serde_json::to_value(&closed).unwrap();
    assert_eq!(v["status"], "resolved");
    assert_eq!(v["resolved_by"]["kind"], "agent");
    assert_eq!(v["resolved_by"]["name"], "claude-code");
    assert!(v["resolved_by"]["at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(w.core.thread(&t, Viewer::Agent).unwrap().comments.len(), 4);

    let kinds = w.event_kinds();
    for k in ["comment.created", "thread.resolved", "thread.unresolved"] {
        assert!(kinds.iter().any(|e| e == k), "{k}: {kinds:?}");
    }
    let err = api::reply(
        &ctx,
        ReplyRequest {
            thread_id: "no-such-thread".into(),
            body_md: "x".into(),
            resolve: None,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::NotFound);
}

#[test]
fn draft_threads_do_not_exist_for_agents() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    let latest = w.core.iterations(&r.review_id).unwrap().pop().unwrap();
    let (repo, _) = w
        .core
        .find_repo_for_diff(latest.diff_id.as_str(), None)
        .unwrap();
    let blobs = polygloss_core::objects::BlobReader::open(&repo).unwrap();
    let draft = w
        .core
        .create_thread(
            &polygloss_core::review::NewThread {
                review_id: r.review_id.clone(),
                diff_id: latest.diff_id.clone(),
                subject: Subject::Review,
                kind: ThreadKind::Comment,
                body_md: "draft".into(),
                author: human(),
            },
            &blobs,
        )
        .unwrap();
    for err in [
        api::reply(
            &ctx,
            ReplyRequest {
                thread_id: draft.clone(),
                body_md: "x".into(),
                resolve: None,
            },
        )
        .map(|_| ())
        .unwrap_err(),
        api::resolve(
            &ctx,
            ResolveRequest {
                thread_id: draft.clone(),
                body_md: None,
            },
        )
        .map(|_| ())
        .unwrap_err(),
    ] {
        assert_eq!(err.code, ApiErrorCode::NotFound, "{err}");
    }
}

// ---- edit_comment, delete_comment ----

#[test]
fn edit_and_delete_own_comments_only() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    let t = api::create_comment(&ctx, note(&r.review_id, "first", None))
        .unwrap()
        .thread_id;
    let root = w.core.thread(&t, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();

    let edited = api::edit_comment(
        &ctx,
        EditCommentRequest {
            comment_id: root.clone(),
            body_md: "second".into(),
        },
    )
    .unwrap();
    assert!(edited.edited_at.ends_with('Z'));
    assert_eq!(
        w.core.comment(&root, Viewer::Agent).unwrap().body_md,
        "second"
    );
    assert!(w.event_kinds().iter().any(|k| k == "comment.edited"));

    // Another client name is not the author (OQ-30), the same name in another
    // session is.
    record_session(&w.core, "sess-cursor", "cursor");
    let cursor = w.ctx_named("cursor", "sess-cursor", no_launch());
    let edit = |ctx: &ApiContext| {
        api::edit_comment(
            ctx,
            EditCommentRequest {
                comment_id: root.clone(),
                body_md: "third".into(),
            },
        )
    };
    assert_eq!(edit(&cursor).unwrap_err().code, ApiErrorCode::Forbidden);
    record_session(&w.core, "sess-two", "claude-code");
    let same_client = w.ctx_named("claude-code", "sess-two", no_launch());
    edit(&same_client).unwrap();
    let delete = |ctx: &ApiContext, id: &str| {
        api::delete_comment(
            ctx,
            DeleteCommentRequest {
                comment_id: id.into(),
            },
        )
    };
    assert_eq!(
        delete(&cursor, &root).unwrap_err().code,
        ApiErrorCode::Forbidden
    );

    // The human's published reply is not the agent's.
    w.core.reply(&t, "human reply", &human()).unwrap();
    w.core
        .submit_review(&r.review_id, Verdict::Comment, "", None)
        .unwrap();
    let human_reply = w
        .core
        .thread(&t, Viewer::Agent)
        .unwrap()
        .comments
        .iter()
        .find(|c| c.author.kind == AuthorKind::Human)
        .unwrap()
        .id
        .clone();
    assert_eq!(
        delete(&ctx, &human_reply).unwrap_err().code,
        ApiErrorCode::Forbidden
    );
    assert_eq!(
        api::edit_comment(
            &ctx,
            EditCommentRequest {
                comment_id: human_reply,
                body_md: "x".into()
            }
        )
        .unwrap_err()
        .code,
        ApiErrorCode::Forbidden
    );

    // Deleting the root that has replies leaves a placeholder.
    let d = delete(&ctx, &root).unwrap();
    assert_eq!(
        serde_json::to_value(&d).unwrap(),
        json!({ "comment_id": root, "deleted": true, "placeholder": true })
    );
    let view = w.core.thread(&t, Viewer::Agent).unwrap();
    assert!(view.comments[0].deleted);
    assert!(w.event_kinds().iter().any(|k| k == "comment.deleted"));

    // A lone comment takes its thread along, without a placeholder.
    let lone = api::create_comment(&ctx, note(&r.review_id, "lone", None))
        .unwrap()
        .thread_id;
    let lone_root = w.core.thread(&lone, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();
    assert!(!delete(&ctx, &lone_root).unwrap().placeholder);
    assert_eq!(
        w.core.thread(&lone, Viewer::Agent).unwrap_err().code(),
        "not_found"
    );
    assert_eq!(
        delete(&ctx, "nope").unwrap_err().code,
        ApiErrorCode::NotFound
    );
}

// ---- request_rereview ----

#[test]
fn request_rereview_pins_a_new_live_iteration_and_launches_hidden() {
    let w = world();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    w.repo.write("b.txt", b"b1\nB2\n");
    let rr = api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: r.review_id.clone(),
            summary_md: "Fixed b".into(),
        },
    )
    .unwrap();
    assert_eq!(rr.iteration, 2);
    assert_ne!(rr.diff_id, r.diff_id);
    assert_eq!(rr.status, "rereview_requested");
    assert_eq!(w.scalar("SELECT status FROM reviews"), "rereview_requested");
    assert_eq!(w.scalar("SELECT rereview_summary FROM reviews"), "Fixed b");
    assert_eq!(
        w.scalar("SELECT pinned_by FROM iterations WHERE seq = 2"),
        "rereview"
    );
    assert!(
        w.event_kinds()
            .iter()
            .any(|k| k == "review.rereview_requested")
    );
    assert_eq!(
        launcher.calls(),
        [(Some(format!("polygloss://review/{}", r.review_id)), false)]
    );

    // Nothing changed since: the same iteration; empty summaries are refused.
    let again = api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: r.review_id.clone(),
            summary_md: "Again".into(),
        },
    )
    .unwrap();
    assert_eq!(again.iteration, 2);
    let err = api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: r.review_id.clone(),
            summary_md: " ".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::Conflict);
    let err = api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: "nope".into(),
            summary_md: "x".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::NotFound);
}

#[test]
fn request_rereview_nudges_a_running_app_and_skips_muted_reviews() {
    let w = world();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    let ops: Ops = Arc::default();
    let app = fake_app(&w.core, ops.clone(), None);
    let req = || RequestRereviewRequest {
        review_id: r.review_id.clone(),
        summary_md: "Fixed".into(),
    };
    api::request_rereview(&ctx, req()).unwrap();
    assert!(launcher.calls().is_empty());
    let ops_now = ops.lock().unwrap().clone();
    assert_eq!(ops_now.len(), 1);
    assert!(matches!(ops_now[0], Op::StoreChanged { .. }));
    drop(app);

    w.core.set_muted(&r.review_id, true).unwrap();
    api::request_rereview(&ctx, req()).unwrap();
    assert!(launcher.calls().is_empty(), "a muted review never launches");
}

#[test]
fn request_rereview_of_a_compare_review_uses_its_latest_iteration() {
    let w = world();
    let launcher = Arc::new(RecordingLauncher {
        fail: true,
        ..RecordingLauncher::default()
    });
    let ctx = w.ctx(launcher);
    let r = api::open_diff(
        &ctx,
        OpenDiffRequest {
            repo: w.repo_arg(),
            source: Some(SourceParam::Compare {
                base: "main".into(),
                head: "feature".into(),
                mode: None,
            }),
            show: Some(false),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap();
    let rr = api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: r.review_id.clone(),
            summary_md: "Again".into(),
        },
    )
    .unwrap();
    assert_eq!((rr.iteration, rr.diff_id.as_str()), (1, r.diff_id.as_str()));
}

// ---- focus ----

#[test]
fn focus_checks_ids_and_sends_the_op() {
    let w = world();
    let launcher = no_launch();
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    let code = |req: FocusRequest| api::focus(&ctx, req).unwrap_err().code;
    assert_eq!(code(FocusRequest::default()), ApiErrorCode::Conflict);
    assert_eq!(
        code(FocusRequest {
            review_id: Some("nope".into()),
            ..FocusRequest::default()
        }),
        ApiErrorCode::NotFound
    );
    assert_eq!(
        code(FocusRequest {
            diff_id: Some("abcdef0123".into()),
            ..FocusRequest::default()
        }),
        ApiErrorCode::NotFound
    );
    assert_eq!(
        code(FocusRequest {
            review_id: Some(r.review_id.clone()),
            line: Some(3),
            ..FocusRequest::default()
        }),
        ApiErrorCode::Conflict
    );
    assert_eq!(
        code(FocusRequest {
            review_id: Some(r.review_id.clone()),
            path: Some("a.txt".into()),
            line: Some(0),
            ..FocusRequest::default()
        }),
        ApiErrorCode::Conflict
    );
    assert_eq!(
        code(FocusRequest {
            review_id: Some(r.review_id.clone()),
            thread_id: Some("nope".into()),
            ..FocusRequest::default()
        }),
        ApiErrorCode::NotFound
    );

    // No app and no launch: unavailable, not an error.
    let req = FocusRequest {
        diff_id: Some(r.diff_id[..12].to_owned()),
        path: Some("a.txt".into()),
        side: Some(SideParam::Old),
        line: Some(5),
        ..FocusRequest::default()
    };
    let res = api::focus(&ctx, req.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(res).unwrap(),
        json!({ "status": "unavailable" })
    );
    assert_eq!(launcher.calls(), [(None, false)]);

    let ops: Ops = Arc::default();
    let app = fake_app(&w.core, ops.clone(), None);
    let res = api::focus(&ctx, req.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(res).unwrap(),
        json!({ "status": "focused" })
    );
    assert_eq!(
        ops.lock().unwrap().clone(),
        [Op::Focus {
            review_id: None,
            diff_id: Some(r.diff_id.clone()),
            path: Some("a.txt".into()),
            side: Some(Side::Old),
            line: Some(5),
            thread_id: None,
        }]
    );
    drop(app);

    // The app's own errors keep their code.
    let _app = fake_app(&w.core, Arc::default(), Some("not_found"));
    assert_eq!(
        api::focus(&ctx, req).unwrap_err().code,
        ApiErrorCode::NotFound
    );
}

#[test]
fn focus_check_validates_ids_without_the_app() {
    // The JSON CLI's `--no-open focus` checks the request like `focus` but
    // never talks to (or launches) the app.
    let w = world();
    let launcher = no_launch();
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    let ops: Ops = Arc::default();
    let _app = fake_app(&w.core, ops.clone(), None);
    let code = |req: FocusRequest| api::focus::check(&ctx, req).unwrap_err().code;
    assert_eq!(code(FocusRequest::default()), ApiErrorCode::Conflict);
    assert_eq!(
        code(FocusRequest {
            review_id: Some("nope".into()),
            ..FocusRequest::default()
        }),
        ApiErrorCode::NotFound
    );
    assert_eq!(
        code(FocusRequest {
            review_id: Some(r.review_id.clone()),
            line: Some(3),
            ..FocusRequest::default()
        }),
        ApiErrorCode::Conflict
    );
    api::focus::check(
        &ctx,
        FocusRequest {
            diff_id: Some(r.diff_id[..12].to_owned()),
            path: Some("a.txt".into()),
            line: Some(5),
            ..FocusRequest::default()
        },
    )
    .unwrap();
    assert!(launcher.calls().is_empty());
    assert!(ops.lock().unwrap().is_empty());
}

#[test]
fn focus_launches_the_app_in_the_background() {
    let w = world();
    let ops: Ops = Arc::default();
    let launcher = Arc::new(RecordingLauncher {
        start_app: Some((w.core.clone(), ops.clone())),
        ..RecordingLauncher::default()
    });
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    let t = api::create_comment(&ctx, note(&r.review_id, "look", None))
        .unwrap()
        .thread_id;
    let res = api::focus(
        &ctx,
        FocusRequest {
            review_id: Some(r.review_id.clone()),
            thread_id: Some(t.clone()),
            ..FocusRequest::default()
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(res).unwrap(),
        json!({ "status": "launched" })
    );
    assert_eq!(launcher.calls(), [(None, false)]);
    assert!(ops.lock().unwrap().iter().any(|op| matches!(
        op,
        Op::Focus { thread_id: Some(id), .. } if *id == t
    )));
}

// ---- nudges ----

#[test]
fn every_write_nudges_a_running_app_once_and_never_launches() {
    let w = world();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    let ops: Ops = Arc::default();
    let _app = fake_app(&w.core, ops.clone(), None);
    let nudges = || {
        ops.lock()
            .unwrap()
            .iter()
            .filter(|op| matches!(op, Op::StoreChanged { .. }))
            .count()
    };

    w.live_review(&ctx);
    assert_eq!(nudges(), 1);
    let t = api::create_comment(&ctx, note(&r.review_id, "n", None))
        .unwrap()
        .thread_id;
    assert_eq!(nudges(), 2);
    let reply = api::reply(
        &ctx,
        ReplyRequest {
            thread_id: t.clone(),
            body_md: "r".into(),
            resolve: None,
        },
    )
    .unwrap();
    assert_eq!(nudges(), 3);
    api::resolve(
        &ctx,
        ResolveRequest {
            thread_id: t.clone(),
            body_md: None,
        },
    )
    .unwrap();
    assert_eq!(nudges(), 4);
    api::unresolve(&ctx, UnresolveRequest { thread_id: t }).unwrap();
    assert_eq!(nudges(), 5);
    api::edit_comment(
        &ctx,
        EditCommentRequest {
            comment_id: reply.comment_id.clone(),
            body_md: "e".into(),
        },
    )
    .unwrap();
    assert_eq!(nudges(), 6);
    api::delete_comment(
        &ctx,
        DeleteCommentRequest {
            comment_id: reply.comment_id,
        },
    )
    .unwrap();
    assert_eq!(nudges(), 7);
    let seqs: Vec<i64> = ops
        .lock()
        .unwrap()
        .iter()
        .filter_map(|op| match op {
            Op::StoreChanged { seq } => Some(*seq),
            _ => None,
        })
        .collect();
    assert!(seqs.windows(2).all(|p| p[0] < p[1]), "{seqs:?}");
    assert!(launcher.calls().is_empty());
}

// ---- T5.9 hardening ----

/// `reply` with `resolve: true` is one transaction: when resolving fails, the
/// reply is not published either (T5.9 #11).
#[test]
fn reply_with_resolve_rolls_back_as_one() {
    let w = world();
    let ctx = w.ctx(no_launch());
    let r = w.live_review(&ctx);
    let t = api::create_comment(&ctx, note(&r.review_id, "n", None))
        .unwrap()
        .thread_id;
    w.core
        .store
        .write(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER block_resolve BEFORE UPDATE OF status ON threads \
                 WHEN NEW.status = 'resolved' BEGIN SELECT RAISE(ABORT, 'resolve blocked'); END;",
            )?;
            Ok(())
        })
        .unwrap();
    let events_before = w.event_kinds().len();
    let err = api::reply(
        &ctx,
        ReplyRequest {
            thread_id: t.clone(),
            body_md: "fixed".into(),
            resolve: Some(true),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::Internal, "{err:?}");
    let view = w.core.thread(&t, Viewer::Agent).unwrap();
    assert_eq!(view.comments.len(), 1, "the reply was rolled back");
    assert_eq!(view.status, ThreadStatus::Open);
    assert_eq!(w.event_kinds().len(), events_before);
}

/// `request_rereview` never launches the app when notifications are off
/// globally (`notifications.enabled: false` in settings.json, T5.9 #12).
#[test]
fn request_rereview_respects_the_global_notifications_setting() {
    let w = world();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = w.ctx(launcher.clone());
    let r = w.live_review(&ctx);
    let config = &w.core.paths.config_dir;
    std::fs::create_dir_all(config).unwrap();
    std::fs::write(
        config.join("settings.json"),
        "{\n  // no notifications\n  \"notifications\": { \"enabled\": false },\n}\n",
    )
    .unwrap();
    api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: r.review_id.clone(),
            summary_md: "Fixed".into(),
        },
    )
    .unwrap();
    assert!(launcher.calls().is_empty(), "{:?}", launcher.calls());

    // Back on: the hidden launch happens.
    std::fs::write(config.join("settings.json"), "{}").unwrap();
    w.repo.write("b.txt", b"b1\nB2\n");
    api::request_rereview(
        &ctx,
        RequestRereviewRequest {
            review_id: r.review_id.clone(),
            summary_md: "Fixed again".into(),
        },
    )
    .unwrap();
    assert_eq!(launcher.calls().len(), 1);
}

/// `focus` checks the location against the diff before it talks to the app
/// (T5.9 #3): a path not in the diff is `not_found`, a side the file does not
/// have is `not_found`, a line past the end of that side is `conflict`.
#[test]
fn focus_rejects_missing_sides_and_lines_past_the_end() {
    let w = world();
    let launcher = no_launch();
    let ctx = w.ctx(launcher.clone());
    w.repo.write("c.txt", b"c1\nc2\n");
    let r = w.live_review(&ctx);
    let ops: Ops = Arc::default();
    let _app = fake_app(&w.core, ops.clone(), None);
    let req = |path: &str, side: SideParam, line: u32| FocusRequest {
        review_id: Some(r.review_id.clone()),
        path: Some(path.into()),
        side: Some(side),
        line: Some(line),
        ..FocusRequest::default()
    };
    let code = |req: FocusRequest| api::focus(&ctx, req).unwrap_err();
    let e = code(req("a.txt", SideParam::New, 11));
    assert_eq!(e.code, ApiErrorCode::Conflict, "{e:?}");
    assert!(e.message.contains("10 lines"), "{}", e.message);
    let e = code(req("c.txt", SideParam::Old, 1));
    assert_eq!(e.code, ApiErrorCode::NotFound, "{e:?}");
    let e = code(req("nope.txt", SideParam::New, 1));
    assert_eq!(e.code, ApiErrorCode::NotFound, "{e:?}");
    assert!(ops.lock().unwrap().is_empty(), "nothing reached the app");
    // The same checks without the app (`--no-open focus`).
    let e = api::focus::check(&ctx, req("a.txt", SideParam::Old, 11)).unwrap_err();
    assert_eq!(e.code, ApiErrorCode::Conflict);
    // In range: sent.
    api::focus(&ctx, req("c.txt", SideParam::New, 2)).unwrap();
    api::focus(&ctx, req("a.txt", SideParam::Old, 10)).unwrap();
    assert_eq!(ops.lock().unwrap().len(), 2);
}

// ---- file categories (T6.9, design §11.15) ----

/// Writes the sandbox's `settings.json`.
fn write_settings(core: &Core, text: &str) {
    let dir = &core.paths.config_dir;
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("settings.json"), text).unwrap();
}

/// `open_diff` of the live worktree since HEAD, without showing it, as JSON.
fn open_live_json(w: &World, ctx: &ApiContext) -> serde_json::Value {
    let r = api::open_diff(
        ctx,
        OpenDiffRequest {
            repo: w.repo_arg(),
            source: Some(SourceParam::Live {
                since: Some("HEAD".into()),
            }),
            show: Some(false),
            ..OpenDiffRequest::default()
        },
    )
    .unwrap();
    serde_json::to_value(&r).unwrap()
}

#[test]
fn open_diff_files_carry_their_category() {
    let w = world();
    let ctx = w.ctx(no_launch());
    write_settings(
        &w.core,
        r#"{
          "categories": {
            "docs": { "enabled": true },
            "custom": [{ "id": "tokens", "name": "Design tokens", "patterns": ["*.tokens.json"] }]
          }
        }"#,
    );
    w.repo.write("Cargo.lock", b"lock\n");
    w.repo.write("docs/x.md", b"# x\n");
    w.repo.write("src/a.test.ts", b"test\n");
    w.repo.write("src/a.ts", b"code\n");
    w.repo.write("ui/colors.tokens.json", b"{}\n");
    let v = open_live_json(&w, &ctx);
    // Git order is unchanged; an uncategorized file has no `category`.
    assert_eq!(
        v["files"],
        json!([
            { "path": "Cargo.lock", "status": "added", "additions": 1, "deletions": 0, "category": "generated" },
            { "path": "docs/x.md", "status": "added", "additions": 1, "deletions": 0, "category": "docs" },
            { "path": "src/a.test.ts", "status": "added", "additions": 1, "deletions": 0, "category": "tests" },
            { "path": "src/a.ts", "status": "added", "additions": 1, "deletions": 0 },
            { "path": "ui/colors.tokens.json", "status": "added", "additions": 1, "deletions": 0, "category": "custom:tokens" },
        ])
    );
}

#[test]
fn open_diff_stats_break_down_categories() {
    let w = world();
    let ctx = w.ctx(no_launch());
    // Git order: Cargo.lock, a.test.ts, a.txt, f000.txt … f196.txt (the first
    // 200), then z.test.ts, the 201st: counted as a file, its lines not.
    w.repo.write("Cargo.lock", b"l1\nl2\nl3\n");
    w.repo.write("a.test.ts", b"t1\nt2\n");
    w.repo.write("a.txt", TEN_EDITED.as_bytes());
    for i in 0..197 {
        w.repo.write(&format!("f{i:03}.txt"), b"f\n");
    }
    w.repo.write("z.test.ts", b"1\n2\n3\n4\n5\n");
    let v = open_live_json(&w, &ctx);
    assert_eq!(
        v["stats"],
        json!({
            "files": 201,
            "additions": 3 + 2 + 1 + 197,
            "deletions": 1,
            "categories": {
                "generated": { "files": 1, "additions": 3, "deletions": 0 },
                "tests": { "files": 2, "additions": 2, "deletions": 0 },
            },
        })
    );
    assert_eq!(v["files_truncated"], true);
    assert_eq!(v["files"].as_array().unwrap().len(), 200);
    assert_eq!(v["files"][199]["path"], "f196.txt");
    // With nothing categorized `stats` has no `categories` key at all
    // (`open_diff_live_pins_assigns_and_counts`).
}

#[test]
fn invalid_settings_use_the_default_categories() {
    let w = world();
    let ctx = w.ctx(no_launch());
    w.repo.write("Cargo.lock", b"lock\n");
    w.repo.write("docs/x.md", b"# x\n");
    w.repo.write("src/a.test.ts", b"test\n");
    let categories = |v: &serde_json::Value| -> Vec<(String, Option<String>)> {
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["path"].as_str().unwrap().to_owned(),
                    f["category"].as_str().map(str::to_owned),
                )
            })
            .collect()
    };
    let owned = |rows: &[(&str, Option<&str>)]| -> Vec<(String, Option<String>)> {
        rows.iter()
            .map(|(p, c)| ((*p).to_owned(), c.map(str::to_owned)))
            .collect()
    };

    // Docs on, but a bad custom id invalidates the whole section: defaults.
    write_settings(
        &w.core,
        r#"{ "categories": { "docs": { "enabled": true }, "custom": [{ "id": "Bad Id", "name": "x" }] } }"#,
    );
    assert_eq!(
        categories(&open_live_json(&w, &ctx)),
        owned(&[
            ("Cargo.lock", Some("generated")),
            ("docs/x.md", None),
            ("src/a.test.ts", Some("tests")),
        ])
    );
    // Unparseable JSON: the defaults too.
    write_settings(&w.core, "{ not json");
    assert_eq!(
        categories(&open_live_json(&w, &ctx))[1],
        ("docs/x.md".to_owned(), None)
    );

    // Fixed: docs apply on the next call.
    write_settings(
        &w.core,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    assert_eq!(
        categories(&open_live_json(&w, &ctx)),
        owned(&[
            ("Cargo.lock", Some("generated")),
            ("docs/x.md", Some("docs")),
            ("src/a.test.ts", Some("tests")),
        ])
    );
}
