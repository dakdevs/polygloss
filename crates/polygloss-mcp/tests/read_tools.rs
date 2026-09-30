//! The read tools and resources (T4.5, design §15.2, §15.3) called through
//! `polygloss_mcp::api` against a sandboxed store: wire shapes the bun suite
//! (`tests/mcp/read-tools.test.ts`) does not reach cheaply — truncation,
//! visibility of drafts, file and review subjects, renames, errors and the
//! resource list.
//!
//! Every test runs under `Sandbox::isolate()`; only nextest (one process per
//! test) is supported.

use std::path::PathBuf;
use std::sync::Arc;

use polygloss_core::ObjectFormat;
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AssignedBy, Author, AuthorKind, Core, NewThread, OpenRequest, OpenedDiff, PinnedBy,
    SessionInfo, Subject, ThreadKind, Verdict,
};
use polygloss_core::store::events::Actor;
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_diff::Side;
use polygloss_mcp::api::{self, GetThreadRequest, ListReviewsRequest, ListThreadsRequest};
use polygloss_mcp::{ApiContext, ApiErrorCode};
use polygloss_platform::launch::Launcher;
use serde_json::{Value, json};

struct NoLaunch;

impl Launcher for NoLaunch {
    fn launch(&self, _url: Option<&str>, _activate: bool) -> std::io::Result<()> {
        panic!("read tools never launch the app");
    }
}

const A_MAIN: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
const A_FEATURE: &str = "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\nl10\n";

struct World {
    _sb: Sandbox,
    repo: FixtureRepo,
    ctx: ApiContext,
    opened: OpenedDiff,
    blobs: BlobReader,
}

/// `main`: `a.txt`, `gone.txt`; `feature`: line 5 of `a.txt` changed, `gone.txt`
/// deleted, `b.txt` added. The review compares them (three-dot).
fn world() -> World {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", A_MAIN.as_bytes());
    repo.write("gone.txt", b"g1\ng2\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", A_FEATURE.as_bytes());
    repo.write("b.txt", b"bee\n");
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
    repo.commit("f1");
    repo.checkout("main");
    let core = Core::open_default().unwrap();
    let opened = open_compare(&core, &repo);
    let blobs = BlobReader::open(&opened.repo).unwrap();
    core.upsert_session(&SessionInfo {
        id: "sess-me".into(),
        client_name: "claude-code".into(),
        client_version: None,
        owner_pid: None,
        cwd: None,
    })
    .unwrap();
    let ctx = ApiContext {
        core,
        session_id: "sess-me".into(),
        client_name: "claude-code".into(),
        launcher: Arc::new(NoLaunch),
        roots: Vec::<PathBuf>::new(),
    };
    World {
        _sb: sb,
        repo,
        ctx,
        opened,
        blobs,
    }
}

fn open_compare(core: &Core, repo: &FixtureRepo) -> OpenedDiff {
    core.open(&OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "main".into(),
            head: "feature".into(),
            mode: CompareMode::ThreeDot,
        },
        label: None,
        pin: Some(PinnedBy::Refresh),
        actor: Actor::human(),
    })
    .unwrap()
}

fn human() -> Author {
    Author {
        kind: AuthorKind::Human,
        name: "you".into(),
        session_id: None,
    }
}

fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: Some("sess-me".into()),
    }
}

impl World {
    fn thread(&self, subject: Subject, kind: ThreadKind, body: &str, author: Author) -> String {
        self.ctx
            .core
            .create_thread(
                &NewThread {
                    review_id: self.opened.review_id.clone(),
                    diff_id: self.opened.diff_id.clone(),
                    subject,
                    kind,
                    body_md: body.into(),
                    author,
                },
                &self.blobs,
            )
            .unwrap()
    }

    fn submit(&self) {
        self.ctx
            .core
            .submit_review(&self.opened.review_id, Verdict::Comment, "", None)
            .unwrap();
    }

    fn get(&self, thread_id: &str) -> Value {
        let r = api::get_thread(
            &self.ctx,
            GetThreadRequest {
                thread_id: thread_id.into(),
                ..GetThreadRequest::default()
            },
        )
        .unwrap();
        serde_json::to_value(r).unwrap()
    }

    fn list(&self, req: ListThreadsRequest) -> Value {
        serde_json::to_value(api::list_threads(&self.ctx, req).unwrap()).unwrap()
    }

    fn review_threads(&self) -> ListThreadsRequest {
        ListThreadsRequest {
            review_id: Some(self.opened.review_id.clone()),
            ..ListThreadsRequest::default()
        }
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

#[test]
fn get_thread_truncates_bodies_over_20k_chars_and_flags_them() {
    let w = world();
    let body = format!("```suggestion\nfixed\n```\n{}", "é".repeat(25_000));
    let id = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Note,
        &body,
        agent(),
    );
    let t = w.get(&id);
    let c = &t["comments"][0];
    assert_eq!(c["truncated"], json!(true));
    // Suggestions come from the whole body, and their replacement and
    // original text count toward the 20k cap (T5.9 #9): "fixed" + "L5".
    assert_eq!(c["body_md"].as_str().unwrap().chars().count(), 20_000 - 7);
    assert_eq!(t["has_suggestion"], json!(true));
    assert_eq!(c["suggestions"][0]["replacement"], json!("fixed"));
    let excerpt = t["last_comment"]["excerpt"].as_str().unwrap();
    assert_eq!(excerpt.chars().count(), 300);
    assert!(excerpt.ends_with('…'), "{excerpt}");
}

#[test]
fn agents_never_see_draft_replies() {
    let w = world();
    let id = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Question,
        "Why?",
        agent(),
    );
    w.ctx.core.reply(&id, "draft answer", &human()).unwrap();
    let t = w.get(&id);
    assert_eq!(t["comment_count"], json!(1));
    assert_eq!(t["comments"].as_array().unwrap().len(), 1);
    assert_eq!(t["last_comment"]["author_kind"], json!("agent"));
    w.submit();
    let t = w.get(&id);
    assert_eq!(t["comment_count"], json!(2));
    assert_eq!(t["last_comment"]["excerpt"], json!("draft answer"));
}

#[test]
fn review_and_file_threads_have_positions_without_lines() {
    let w = world();
    let review = w.thread(Subject::Review, ThreadKind::Note, "Overall.", agent());
    let file = w.thread(
        Subject::File {
            path: "b.txt".into(),
        },
        ThreadKind::Note,
        "New file.",
        agent(),
    );
    let t = w.get(&review);
    assert_eq!(t["subject"], json!("review"));
    assert_eq!(t["position"], json!({ "state": "exact" }));
    assert!(t.get("path").is_none(), "{t}");
    assert_eq!(t["anchor"], json!({}));

    let t = w.get(&file);
    assert_eq!(t["subject"], json!("file"));
    assert_eq!(t["path"], json!("b.txt"));
    assert_eq!(t["position"], json!({ "state": "exact" }));
    assert_eq!(t["anchor"], json!({ "path": "b.txt" }));
}

#[test]
fn old_side_and_added_file_hunks_end_at_the_commented_line() {
    let w = world();
    let old = w.thread(
        line("a.txt", Side::Old, 5, 5),
        ThreadKind::Note,
        "was l5",
        agent(),
    );
    let t = w.get(&old);
    assert_eq!(t["anchor"]["original_snippet"], json!("l5"));
    assert_eq!(
        t["anchor"]["diff_hunk"],
        json!("@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5")
    );

    let added = w.thread(
        line("b.txt", Side::New, 1, 1),
        ThreadKind::Note,
        "bee",
        agent(),
    );
    let t = w.get(&added);
    assert_eq!(t["anchor"]["diff_hunk"], json!("@@ -0,0 +1 @@\n+bee"));

    let deleted = w.thread(
        line("gone.txt", Side::Old, 1, 2),
        ThreadKind::Note,
        "why?",
        agent(),
    );
    let t = w.get(&deleted);
    assert_eq!(t["anchor"]["original_snippet"], json!("g1\ng2"));
    assert_eq!(t["anchor"]["diff_hunk"], json!("@@ -1,2 +0,0 @@\n-g1\n-g2"));
    assert_eq!(t["comments"][0]["suggestions"], json!([]));
}

#[test]
fn context_line_outside_any_hunk_gets_a_context_hunk() {
    let w = world();
    let id = w.thread(
        line("a.txt", Side::New, 10, 10),
        ThreadKind::Note,
        "end",
        agent(),
    );
    let t = w.get(&id);
    assert_eq!(
        t["anchor"]["diff_hunk"],
        json!("@@ -7,4 +7,4 @@\n l7\n l8\n l9\n l10")
    );
}

#[test]
fn positions_follow_a_renamed_file() {
    let w = world();
    let id = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Note,
        "```suggestion\nL5!\n```",
        agent(),
    );
    w.repo.checkout("feature");
    w.repo.git(&["mv", "a.txt", "z.txt"]);
    w.repo.commit("rename");
    w.repo.checkout("main");
    let second = open_compare(&w.ctx.core, &w.repo);
    assert_ne!(second.diff_id, w.opened.diff_id);

    let t = w.get(&id);
    assert_eq!(t["path"], json!("a.txt"));
    assert_eq!(
        t["position"],
        json!({ "state": "exact", "path": "z.txt", "start_line": 5, "line": 5 })
    );
    assert_eq!(t["anchor"]["current_snippet"], json!("L5"));
    assert_eq!(
        t["comments"][0]["suggestions"],
        json!([{ "start_line": 5, "line": 5, "original": "L5", "replacement": "L5!" }])
    );
    // Relative to the first iteration when its diff id is passed.
    let listed = w.list(ListThreadsRequest {
        diff_id: Some(w.opened.diff_id.as_str().to_owned()),
        ..w.review_threads()
    });
    assert_eq!(
        listed["threads"][0]["position"],
        json!({ "state": "exact", "start_line": 5, "line": 5 })
    );
}

#[test]
fn list_threads_filters_by_kind_author_status_and_path() {
    let w = world();
    let q = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Question,
        "q",
        agent(),
    );
    let n = w.thread(Subject::Review, ThreadKind::Note, "n", agent());
    let h = w.thread(
        line("b.txt", Side::New, 1, 1),
        ThreadKind::Comment,
        "h",
        human(),
    );
    w.submit();
    w.ctx
        .core
        .set_resolved(&n, true, &agent().actor(), None)
        .expect("resolve the note");
    let ids = |v: Value| -> Vec<String> {
        v["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["thread_id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(w.list(w.review_threads())), [q.clone(), h.clone()]);
    let all = |f: fn(&mut ListThreadsRequest)| {
        let mut r = ListThreadsRequest {
            status: Some(api::shapes::ThreadStatusFilter::All),
            ..w.review_threads()
        };
        f(&mut r);
        ids(w.list(r))
    };
    assert_eq!(all(|_| {}), [q.clone(), n.clone(), h.clone()]);
    assert_eq!(
        all(|r| r.status = Some(api::shapes::ThreadStatusFilter::Resolved)),
        std::slice::from_ref(&n)
    );
    assert_eq!(
        all(|r| r.author = Some(api::shapes::AuthorFilter::Human)),
        std::slice::from_ref(&h)
    );
    assert_eq!(
        all(|r| r.kind = Some(api::shapes::ThreadKindParam::Question)),
        std::slice::from_ref(&q)
    );
    assert_eq!(
        all(|r| r.path = Some("b.txt".into())),
        std::slice::from_ref(&h)
    );
    let resolved = w.get(&n);
    assert_eq!(resolved["status"], json!("resolved"));
    assert_eq!(resolved["resolved_by"]["kind"], json!("agent"));
    assert_eq!(resolved["resolved_by"]["name"], json!("claude-code"));
}

#[test]
fn list_threads_errors() {
    let w = world();
    let err = api::list_threads(&w.ctx, ListThreadsRequest::default()).unwrap_err();
    assert_eq!(err.code, ApiErrorCode::Conflict);
    let err = api::list_threads(
        &w.ctx,
        ListThreadsRequest {
            review_id: Some("0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into()),
            ..ListThreadsRequest::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::NotFound);
    let err = api::list_threads(
        &w.ctx,
        ListThreadsRequest {
            diff_id: Some("abc".into()),
            ..ListThreadsRequest::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::NotFound);
    let err = api::get_thread(
        &w.ctx,
        GetThreadRequest {
            thread_id: "nope".into(),
            ..GetThreadRequest::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::NotFound);
}

#[test]
fn list_reviews_shapes_and_errors() {
    let w = world();
    w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Question,
        "q",
        agent(),
    );
    let v = serde_json::to_value(api::list_reviews(&w.ctx, ListReviewsRequest::default()).unwrap())
        .unwrap();
    let r = &v["reviews"][0];
    assert_eq!(r["review_id"], json!(w.opened.review_id));
    assert_eq!(r["kind"], json!("compare"));
    assert_eq!(r["status"], json!("open"));
    assert_eq!(r["iterations"], json!(1));
    assert_eq!(r["open_questions"], json!(1));
    assert_eq!(r["viewed"], json!({ "done": 0, "total": 3 }));
    assert!(r.get("last_submission").is_none(), "{r}");
    assert!(r.get("rereview").is_none(), "{r}");
    assert!(r.get("assigned_session").is_none(), "{r}");
    assert!(v.get("next_cursor").is_none(), "{v}");

    let outside = tempfile::tempdir().unwrap();
    let err = api::list_reviews(
        &w.ctx,
        ListReviewsRequest {
            repo: Some(outside.path().to_string_lossy().into_owned()),
            ..ListReviewsRequest::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::RepoNotFound);
    let err = api::list_reviews(
        &w.ctx,
        ListReviewsRequest {
            cursor: Some("garbage".into()),
            ..ListReviewsRequest::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::Conflict);
}

#[test]
fn resources_list_dedupes_assigned_and_recent() {
    let w = world();
    w.ctx
        .core
        .assign_review(&w.opened.review_id, "sess-me", AssignedBy::OpenDiff)
        .unwrap();
    let list = api::resources::list_resources(&w.ctx).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0].uri,
        format!("polygloss://review/{}", w.opened.review_id)
    );
    assert!(!list[0].name.is_empty());
    let templates = api::resources::resource_templates();
    assert_eq!(templates.len(), 4);
}

#[test]
fn read_resource_rejects_unknown_uris() {
    let w = world();
    for uri in [
        "polygloss://review/nope",
        "polygloss://thread/nope",
        "polygloss://diff/abc",
        "polygloss://other/x",
        "https://example.com/",
        "polygloss://review/a/b/c",
    ] {
        let err = api::resources::read_resource(&w.ctx, uri).unwrap_err();
        assert_eq!(err.code, ApiErrorCode::NotFound, "{uri}");
    }
    let md = api::resources::read_resource(
        &w.ctx,
        &format!("polygloss://diff/{}", &w.opened.diff_id.as_str()[..10]),
    )
    .unwrap();
    assert!(md.contains("b.txt"), "{md}");
    assert!(md.contains("added"), "{md}");
    assert!(md.contains("gone.txt"), "{md}");
    assert!(md.contains("deleted"), "{md}");
}

/// A comment whose suggestions alone exceed the cap keeps its (cut) body but
/// not the suggestions, which would be incomplete; `truncated` says so (T5.9 #9).
#[test]
fn suggestions_too_large_for_the_cap_are_left_out() {
    let w = world();
    let big = "x".repeat(21_000);
    let body = format!("Replace it:\n```suggestion\n{big}\n```\n");
    let id = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Note,
        &body,
        agent(),
    );
    let t = w.get(&id);
    let c = &t["comments"][0];
    assert_eq!(c["truncated"], json!(true));
    assert_eq!(c["suggestions"], json!([]));
    let chars = c["body_md"].as_str().unwrap().chars().count();
    assert_eq!(chars, 20_000);
}

/// `get_thread` stays under the page budget however many long comments a
/// thread has: it pages its comments with `next_cursor` and flags
/// `comments_truncated` (T5.9 #9).
#[test]
fn get_thread_pages_comments_to_stay_under_the_budget() {
    let w = world();
    let id = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Note,
        "Start.",
        agent(),
    );
    for i in 0..12 {
        w.ctx
            .core
            .reply(
                &id,
                &format!("{i}: {}", "long reply ".repeat(1_500)),
                &agent(),
            )
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let r = api::get_thread(
            &w.ctx,
            GetThreadRequest {
                thread_id: id.clone(),
                cursor: cursor.clone(),
            },
        )
        .unwrap();
        let v = serde_json::to_value(&r).unwrap();
        let size = v.to_string().chars().count();
        assert!(size < polygloss_mcp::PAGE_MAX_CHARS, "{size}");
        assert_eq!(v["comment_count"], json!(13));
        for c in v["comments"].as_array().unwrap() {
            seen.push(c["comment_id"].as_str().unwrap().to_owned());
        }
        cursor = v["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(
            v["comments_truncated"].as_bool().unwrap_or(false),
            cursor.is_some()
        );
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none(), "paging never ended");
    assert_eq!(seen.len(), 13);
    let unique: std::collections::BTreeSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 13);

    // The thread resource stays under the budget too and says how to read on.
    let md = api::resources::read_resource(&w.ctx, &format!("polygloss://thread/{id}")).unwrap();
    assert!(md.chars().count() < polygloss_mcp::PAGE_MAX_CHARS);
    assert!(md.contains("next_cursor"), "{}", &md[md.len() - 300..]);

    // A cursor from another thread is refused.
    let other = w.thread(Subject::Review, ThreadKind::Note, "Other.", agent());
    let first = api::get_thread(
        &w.ctx,
        GetThreadRequest {
            thread_id: id.clone(),
            ..GetThreadRequest::default()
        },
    )
    .unwrap();
    let err = api::get_thread(
        &w.ctx,
        GetThreadRequest {
            thread_id: other,
            cursor: first.next_cursor.clone(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::Conflict);
}

/// The review's repo is gone (deleted or moved): thread reads still answer,
/// with positions `absent` (or cached ones) instead of `repo_not_found`
/// (T5.9 #7).
#[test]
fn thread_reads_survive_a_deleted_repo() {
    let w = world();
    let id = w.thread(
        line("a.txt", Side::New, 5, 5),
        ThreadKind::Question,
        "Why?",
        agent(),
    );
    let cached = w.thread(
        line("a.txt", Side::New, 4, 4),
        ThreadKind::Note,
        "Context.",
        agent(),
    );
    // Cache one thread's position, then forget the other's.
    assert_eq!(w.get(&cached)["position"]["state"], json!("exact"));
    w.ctx
        .core
        .store
        .write(|tx| Ok(tx.execute("DELETE FROM thread_positions WHERE thread_id = ?1", [&id])?))
        .unwrap();
    std::fs::remove_dir_all(w.repo.path()).unwrap();

    let t = w.get(&id);
    assert_eq!(t["position"], json!({ "state": "absent" }));
    assert_eq!(t["anchor"]["original_snippet"], json!("L5"));
    assert!(t["anchor"].get("current_snippet").is_none(), "{t}");
    assert_eq!(w.get(&cached)["position"]["state"], json!("exact"));

    let list = w.list(w.review_threads());
    let states: Vec<&str> = list["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["position"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["absent", "exact"]);
    for uri in [
        format!("polygloss://thread/{id}"),
        format!("polygloss://review/{}/threads", w.opened.review_id),
        format!("polygloss://review/{}", w.opened.review_id),
    ] {
        let md = api::resources::read_resource(&w.ctx, &uri).unwrap();
        assert!(md.contains("Why?") || md.contains("Review"), "{uri}: {md}");
    }
}
