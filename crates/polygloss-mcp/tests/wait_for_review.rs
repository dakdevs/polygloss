//! `wait_for_review` (T4.7, design §15.2) called through `polygloss_mcp::api`
//! against a sandboxed store: what the bun suite
//! (`tests/mcp/wait-for-review.test.ts`) does not reach cheaply — which
//! submission is reported, archive and prune, cancellation and progress through
//! [`WaitControl`], the thread list, truncation and the timeout rules.
//!
//! Every test runs under `Sandbox::isolate()`; only nextest (one process per
//! test) is supported.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use polygloss_core::ObjectFormat;
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, OpenedDiff, PinnedBy, SessionInfo, Subject,
    ThreadKind, Verdict,
};
use polygloss_core::store::events::{Actor, latest_seq};
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_diff::Side;
use polygloss_mcp::api::shapes::ThreadStatusFilter;
use polygloss_mcp::api::wait_for_review::{
    DEFAULT_TIMEOUT_S, MAX_TIMEOUT_S, PROGRESS_ENV, PROGRESS_EVERY, WaitTiming, effective_timeout,
    wait_for_review_with,
};
use polygloss_mcp::api::{self, ListThreadsRequest, WaitControl, WaitForReviewRequest};
use polygloss_mcp::{ApiContext, ApiErrorCode};
use polygloss_platform::launch::Launcher;
use serde_json::{Value, json};

struct NoLaunch;

impl Launcher for NoLaunch {
    fn launch(&self, _url: Option<&str>, _activate: bool) -> std::io::Result<()> {
        panic!("wait_for_review never launches the app");
    }
}

const RFC3339_LEN: usize = "2026-09-29T10:11:12.345Z".len();

struct World {
    _sb: Sandbox,
    repo: FixtureRepo,
    ctx: ApiContext,
    opened: OpenedDiff,
    blobs: BlobReader,
}

/// A compare review (`main...feature`, `a.txt` line 5 changed) with one iteration.
fn world() -> World {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"l1\nl2\nl3\nl4\nl5\nl6\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", b"l1\nl2\nl3\nl4\nL5\nl6\n");
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

/// Fast timing for tests: poll every 20 ms, progress every 50 ms.
fn fast() -> WaitTiming {
    WaitTiming {
        poll: Duration::from_millis(20),
        progress_every: Duration::from_millis(50),
    }
}

impl World {
    fn review(&self) -> String {
        self.opened.review_id.clone()
    }

    fn seq(&self) -> i64 {
        self.ctx.core.store.read(latest_seq).unwrap()
    }

    fn thread(&self, line: u32, kind: ThreadKind, body: &str, author: Author) -> String {
        self.ctx
            .core
            .create_thread(
                &NewThread {
                    review_id: self.review(),
                    diff_id: self.opened.diff_id.clone(),
                    subject: Subject::Line {
                        path: "a.txt".into(),
                        side: Side::New,
                        start_line: line,
                        line,
                    },
                    kind,
                    body_md: body.into(),
                    author,
                },
                &self.blobs,
            )
            .unwrap()
    }

    fn submit(&self, verdict: Verdict, summary: &str) -> polygloss_core::review::Submission {
        self.ctx
            .core
            .submit_review(&self.review(), verdict, summary, None)
            .unwrap()
    }

    fn wait(&self, since: Option<i64>, timeout_s: Option<u32>) -> Value {
        self.wait_ctl(since, timeout_s, &WaitControl::default())
    }

    fn wait_ctl(&self, since: Option<i64>, timeout_s: Option<u32>, ctl: &WaitControl) -> Value {
        let r = wait_for_review_with(
            &self.ctx,
            WaitForReviewRequest {
                review_id: self.review(),
                since,
                timeout_s,
            },
            ctl,
            &fast(),
        )
        .unwrap();
        serde_json::to_value(r).unwrap()
    }
}

#[test]
fn returns_at_once_when_a_submission_exists_after_since() {
    let w = world();
    let since = w.seq();
    let s = w.submit(Verdict::RequestChanges, "Rename the helper.");
    let started = Instant::now();
    let r = w.wait(Some(since), None);
    assert!(started.elapsed() < Duration::from_secs(2), "{r}");
    assert_eq!(r["outcome"], json!("submitted"));
    let sub = &r["submission"];
    assert_eq!(sub["submission_id"], json!(s.id));
    assert_eq!(sub["verdict"], json!("request_changes"));
    assert_eq!(sub["summary_md"], json!("Rename the helper."));
    assert_eq!(sub["iteration"], json!(1));
    assert_eq!(sub["at"].as_str().unwrap().len(), RFC3339_LEN, "{sub}");
    assert!(sub.get("summary_truncated").is_none(), "{sub}");
    assert_eq!(r["threads"], json!([]));
    assert!(r.get("threads_truncated").is_none(), "{r}");
    assert_eq!(r["next_since"], json!(w.seq()));
    assert!(r["next_since"].as_i64().unwrap() >= s.seq);
}

#[test]
fn the_latest_submission_after_since_is_reported() {
    let w = world();
    let since = w.seq();
    w.submit(Verdict::Comment, "first");
    let second = w.submit(Verdict::Approve, "second");
    let r = w.wait(Some(since), Some(0));
    assert_eq!(r["outcome"], json!("submitted"));
    assert_eq!(r["submission"]["submission_id"], json!(second.id));
    assert_eq!(r["submission"]["verdict"], json!("approve"));
}

#[test]
fn default_since_is_now_so_earlier_submissions_do_not_count() {
    let w = world();
    w.submit(Verdict::Comment, "old");
    let r = w.wait(None, Some(0));
    assert_eq!(r, json!({ "outcome": "timeout", "next_since": w.seq() }));
}

#[test]
fn next_since_of_a_submission_waits_for_the_next_one() {
    let w = world();
    let since = w.seq();
    w.submit(Verdict::Comment, "one");
    let r = w.wait(Some(since), Some(0));
    let next = r["next_since"].as_i64().unwrap();
    let again = w.wait(Some(next), Some(0));
    assert_eq!(again["outcome"], json!("timeout"));
    assert_eq!(again["next_since"], json!(next));
}

#[test]
fn blocks_until_another_connection_submits() {
    let w = world();
    let since = w.seq();
    let core = Core::open_default().unwrap();
    let review = w.review();
    let submitter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        core.submit_review(&review, Verdict::Comment, "later", None)
            .unwrap()
    });
    let started = Instant::now();
    let r = w.wait(Some(since), Some(30));
    let s = submitter.join().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250), "{r}");
    assert!(started.elapsed() < Duration::from_secs(10), "{r}");
    assert_eq!(r["outcome"], json!("submitted"));
    assert_eq!(r["submission"]["submission_id"], json!(s.id));
}

#[test]
fn other_reviews_and_other_events_do_not_end_the_wait() {
    let w = world();
    let since = w.seq();
    // An agent note and another review's submission are not this review's
    // submission.
    w.thread(5, ThreadKind::Note, "note", agent());
    w.repo.write("a.txt", b"live\n");
    let other = w
        .ctx
        .core
        .open(&OpenRequest {
            worktree: w.repo.path().to_path_buf(),
            source: Source::Live {
                since: polygloss_core::git::Since::Head,
            },
            label: None,
            pin: Some(PinnedBy::Manual),
            actor: Actor::human(),
        })
        .unwrap();
    assert_ne!(other.review_id, w.review());
    w.ctx
        .core
        .submit_review(&other.review_id, Verdict::Approve, "", None)
        .unwrap();
    let r = w.wait(Some(since), Some(0));
    assert_eq!(r, json!({ "outcome": "timeout", "next_since": w.seq() }));
}

#[test]
fn timeout_returns_outcome_timeout_with_next_since() {
    let w = world();
    let started = Instant::now();
    let r = w.wait(None, Some(1));
    let took = started.elapsed();
    assert!(took >= Duration::from_secs(1), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(r, json!({ "outcome": "timeout", "next_since": w.seq() }));
}

#[test]
fn archived_during_the_wait_returns_outcome_archived() {
    let w = world();
    let since = w.seq();
    let core = w.ctx.core.clone();
    let review = w.review();
    let archiver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        core.archive_review(&review, &Actor::human()).unwrap();
    });
    let r = w.wait(Some(since), Some(30));
    archiver.join().unwrap();
    assert_eq!(r, json!({ "outcome": "archived", "next_since": w.seq() }));
}

#[test]
fn an_archived_review_returns_outcome_archived_at_once() {
    let w = world();
    w.ctx
        .core
        .archive_review(&w.review(), &Actor::human())
        .unwrap();
    let started = Instant::now();
    let r = w.wait(None, Some(30));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(r, json!({ "outcome": "archived", "next_since": w.seq() }));
}

#[test]
fn a_pruned_review_returns_outcome_archived() {
    let w = world();
    let since = w.seq();
    w.ctx.core.prune_review(&w.review()).unwrap();
    let r = w.wait(Some(since), Some(0));
    assert_eq!(r["outcome"], json!("archived"));
}

#[test]
fn a_submission_wins_over_a_later_archive() {
    let w = world();
    let since = w.seq();
    w.submit(Verdict::Approve, "done");
    w.ctx
        .core
        .archive_review(&w.review(), &Actor::human())
        .unwrap();
    let r = w.wait(Some(since), Some(0));
    assert_eq!(r["outcome"], json!("submitted"));
    // The next call reports the archive.
    let next = r["next_since"].as_i64().unwrap();
    assert_eq!(w.wait(Some(next), Some(0))["outcome"], json!("archived"));
}

#[test]
fn cancellation_ends_the_wait() {
    let w = world();
    let ctl = WaitControl::default();
    let flag = ctl.cancel_flag();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let started = Instant::now();
    let r = wait_for_review_with(
        &w.ctx,
        WaitForReviewRequest {
            review_id: w.review(),
            since: None,
            timeout_s: Some(60),
        },
        &ctl,
        &fast(),
    );
    canceller.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let err = r.unwrap_err();
    assert!(err.message.contains("cancelled"), "{err:?}");
}

#[test]
fn progress_is_reported_every_interval_while_waiting() {
    let w = world();
    let seen: Arc<Mutex<Vec<(Duration, Duration)>>> = Arc::default();
    let sink = seen.clone();
    let ctl = WaitControl::with_progress(Box::new(move |elapsed, total| {
        sink.lock().unwrap().push((elapsed, total));
    }));
    let r = w.wait_ctl(None, Some(1), &ctl);
    assert_eq!(r["outcome"], json!("timeout"));
    let seen = seen.lock().unwrap();
    // One second at 50 ms: many reports, never faster than the interval.
    assert!(seen.len() >= 5, "{seen:?}");
    assert!(seen.len() <= 21, "{seen:?}");
    for pair in seen.windows(2) {
        assert!(pair[1].0 > pair[0].0, "{seen:?}");
    }
    assert!(
        seen.iter()
            .all(|(_, total)| *total == Duration::from_secs(1))
    );
}

#[test]
fn no_progress_without_a_listener_or_before_the_interval() {
    let w = world();
    let seen: Arc<Mutex<u32>> = Arc::default();
    let sink = seen.clone();
    let ctl = WaitControl::with_progress(Box::new(move |_, _| {
        *sink.lock().unwrap() += 1;
    }));
    let since = w.seq();
    w.submit(Verdict::Comment, "");
    let r = w.wait_ctl(Some(since), Some(1), &ctl);
    assert_eq!(r["outcome"], json!("submitted"));
    assert_eq!(*seen.lock().unwrap(), 0);
}

#[test]
fn threads_lists_new_or_updated_threads_since() {
    let w = world();
    let untouched = w.thread(2, ThreadKind::Note, "untouched", agent());
    let answered = w.thread(3, ThreadKind::Question, "Why?", agent());
    let since = w.seq();
    w.ctx.core.reply(&answered, "Because.", &human()).unwrap();
    let fresh = w.thread(5, ThreadKind::Comment, "Rename this.", human());
    w.submit(Verdict::RequestChanges, "");
    let r = w.wait(Some(since), Some(0));
    assert_eq!(r["outcome"], json!("submitted"));
    let ids: Vec<&str> = r["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["thread_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [answered.as_str(), fresh.as_str()]);
    assert!(!ids.contains(&untouched.as_str()));
    // The same summaries list_threads gives for the same `since`.
    let listed = api::list_threads(
        &w.ctx,
        ListThreadsRequest {
            review_id: Some(w.review()),
            since: Some(since),
            status: Some(ThreadStatusFilter::All),
            ..ListThreadsRequest::default()
        },
    )
    .unwrap();
    assert_eq!(r["threads"], serde_json::to_value(&listed.threads).unwrap());
    assert_eq!(
        r["threads"][1]["last_comment"]["author_kind"],
        json!("human")
    );
}

#[test]
fn resolved_threads_count_as_updated() {
    let w = world();
    let t = w.thread(2, ThreadKind::Note, "note", agent());
    let since = w.seq();
    w.ctx
        .core
        .set_resolved(&t, true, &human().actor(), None)
        .unwrap();
    w.submit(Verdict::Comment, "");
    let r = w.wait(Some(since), Some(0));
    assert_eq!(r["threads"][0]["thread_id"], json!(t));
    assert_eq!(r["threads"][0]["status"], json!("resolved"));
}

#[test]
fn long_summaries_are_truncated_and_flagged() {
    let w = world();
    let since = w.seq();
    w.submit(Verdict::Comment, &"é".repeat(25_000));
    let r = w.wait(Some(since), Some(0));
    let sub = &r["submission"];
    assert_eq!(sub["summary_truncated"], json!(true));
    assert_eq!(sub["summary_md"].as_str().unwrap().chars().count(), 20_000);
}

#[test]
fn many_threads_stay_under_the_page_budget_and_are_flagged() {
    let w = world();
    let since = w.seq();
    let long = "x".repeat(2_000);
    for i in 0..150 {
        w.thread(
            1 + (i % 6),
            ThreadKind::Comment,
            &format!("{i} {long}"),
            human(),
        );
    }
    w.submit(Verdict::Comment, &"s".repeat(15_000));
    let r = w.wait(Some(since), Some(0));
    let text = serde_json::to_string(&r).unwrap();
    assert!(text.chars().count() < 60_000, "{}", text.len());
    assert_eq!(r["threads_truncated"], json!(true));
    let n = r["threads"].as_array().unwrap().len();
    assert!(n > 0 && n < 150, "{n}");
}

#[test]
fn unknown_review_is_not_found() {
    let w = world();
    let err = wait_for_review_with(
        &w.ctx,
        WaitForReviewRequest {
            review_id: "no-such-review".into(),
            since: None,
            timeout_s: Some(0),
        },
        &WaitControl::default(),
        &fast(),
    )
    .unwrap_err();
    assert_eq!(err.code, ApiErrorCode::NotFound);
}

#[test]
fn timeout_defaults_to_and_is_clamped_at_1500_seconds() {
    assert_eq!(DEFAULT_TIMEOUT_S, 1500);
    assert_eq!(MAX_TIMEOUT_S, 1500);
    assert_eq!(effective_timeout(None), Duration::from_secs(1500));
    assert_eq!(effective_timeout(Some(0)), Duration::ZERO);
    assert_eq!(effective_timeout(Some(30)), Duration::from_secs(30));
    assert_eq!(effective_timeout(Some(99_999)), Duration::from_secs(1500));
}

#[test]
fn progress_interval_override_needs_test_mode() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    assert_eq!(PROGRESS_EVERY, Duration::from_secs(60));
    let default = WaitTiming::from_env(env(&[]));
    assert_eq!(default.progress_every, PROGRESS_EVERY);
    assert_eq!(default.poll, Duration::from_millis(250));
    let no_test_mode = WaitTiming::from_env(env(&[(PROGRESS_ENV, "200")]));
    assert_eq!(no_test_mode.progress_every, PROGRESS_EVERY);
    let test_mode = WaitTiming::from_env(env(&[(PROGRESS_ENV, "200"), ("POLYGLOSS_TEST", "1")]));
    assert_eq!(test_mode.progress_every, Duration::from_millis(200));
    for bad in ["0", "-5", "soon", ""] {
        let t = WaitTiming::from_env(move |k: &str| match k {
            "POLYGLOSS_TEST" => Some("1".to_owned()),
            k if k == PROGRESS_ENV => Some(bad.to_owned()),
            _ => None,
        });
        assert_eq!(t.progress_every, PROGRESS_EVERY, "{bad}");
    }
}

/// A `since` above the latest seq is clamped to it: a submission appended
/// afterwards is reported, not skipped until the seq catches up (T5.9 #8).
#[test]
fn a_since_above_the_latest_seq_is_clamped() {
    let w = world();
    let future = w.seq() + 1_000;
    let core = Core::open_default().unwrap();
    let review = w.review();
    let submitter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        core.submit_review(&review, Verdict::Comment, "after", None)
            .unwrap()
    });
    let r = w.wait(Some(future), Some(10));
    let s = submitter.join().unwrap();
    assert_eq!(r["outcome"], json!("submitted"), "{r}");
    assert_eq!(r["submission"]["submission_id"], json!(s.id));
    assert!(r["next_since"].as_i64().unwrap() < future, "{r}");
}

/// The review's repo is gone (moved or deleted): the wait still reports the
/// submission and its threads, positioned `absent` where nothing is cached,
/// instead of failing with `repo_not_found` (T5.9 #7).
#[test]
fn a_review_whose_repo_is_gone_still_reports_its_threads() {
    let w = world();
    let since = w.seq();
    let t = w.thread(5, ThreadKind::Comment, "Why?", human());
    w.submit(Verdict::RequestChanges, "fix");
    w.ctx
        .core
        .store
        .write(|tx| Ok(tx.execute("DELETE FROM thread_positions", [])?))
        .unwrap();
    std::fs::remove_dir_all(w.repo.path()).unwrap();
    let r = w.wait(Some(since), Some(5));
    assert_eq!(r["outcome"], json!("submitted"), "{r}");
    let threads = r["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 1, "{r}");
    assert_eq!(threads[0]["thread_id"], json!(t));
    assert_eq!(threads[0]["position"]["state"], json!("absent"));
}
