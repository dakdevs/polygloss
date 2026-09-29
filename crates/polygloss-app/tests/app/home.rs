//! Home and recents (T3.4, design §11.2, §17, OQ-32, OQ-34): the two
//! sections, what a row shows, the row actions (open, archive, prune, mute,
//! assign to session) and the automatic prune at launch.

use std::path::Path;
use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use polygloss_app::home::HomeView;
use polygloss_app::home::prune::{AutoPrune, PRUNE_INTERVAL};
use polygloss_app::home::row::{
    self, HomeRow, ParsedKey, open_request, parse_key, relative_time, status_label,
};
use polygloss_app::tabs::TabItem;
use polygloss_core::git::{CompareMode, ReviewKind, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::threads::{Author, AuthorKind, NewThread, Subject, ThreadKind};
use polygloss_core::review::{
    AssignedBy, Core, OpenRequest, OpenedDiff, ReviewFilter, ReviewSummary, SessionInfo, Verdict,
};
use polygloss_core::store::events::{Actor, ActorKind, now_ms};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

const HOUR: i64 = 60 * 60 * 1000;
const DAY: i64 = 24 * HOUR;

fn commit_req(repo: &Path, rev: &str) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Commit { rev: rev.into() },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn agent() -> Actor {
    Actor {
        kind: ActorKind::Agent,
        name: Some("claude-code".into()),
        session_id: None,
    }
}

/// Sets the review's last activity.
fn set_updated_at(core: &Core, review_id: &str, at: i64) {
    core.store
        .write(|tx| {
            tx.execute(
                &format!("UPDATE reviews SET updated_at = {at} WHERE id = '{review_id}'"),
                (),
            )?;
            Ok(())
        })
        .expect("set updated_at");
}

fn home(shell: &mut Shell) -> Entity<HomeView> {
    match shell.tab(0) {
        TabItem::Home(home) => home,
        TabItem::Review(_) => panic!("tab 0 is Home"),
    }
}

/// Reloads Home and waits for it.
fn reload(shell: &mut Shell) -> Entity<HomeView> {
    let home = home(shell);
    home.update(shell.cx, |h, cx| h.refresh(cx));
    draw(shell.cx);
    home
}

fn ids(rows: &[HomeRow]) -> Vec<String> {
    rows.iter().map(|r| r.review_id().to_owned()).collect()
}

/// `(awaiting you, recent)` review ids.
fn sections(shell: &mut Shell) -> (Vec<String>, Vec<String>) {
    let home = reload(shell);
    home.read_with(shell.cx, |h, _| {
        assert!(h.is_loaded());
        (ids(h.awaiting_you()), ids(h.recent()))
    })
}

fn row_of(shell: &mut Shell, review_id: &str) -> HomeRow {
    let home = home(shell);
    home.read_with(shell.cx, |h, _| {
        h.rows()
            .iter()
            .find(|r| r.review_id() == review_id)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {review_id}"))
    })
}

fn summary(core: &Core, review_id: &str) -> Option<ReviewSummary> {
    core.review_summaries(&ReviewFilter {
        include_archived: true,
        ..ReviewFilter::default()
    })
    .unwrap()
    .into_iter()
    .find(|s| s.review_id == review_id)
}

/// Selects the row of `review_id` (as `j`/`k` would).
fn select(shell: &mut Shell, review_id: &str) {
    let home = home(shell);
    home.update(shell.cx, |h, cx| h.select(Some(review_id), cx));
    draw(shell.cx);
}

fn painted_top(shell: &mut Shell, review_id: &str) -> f32 {
    let selector: &'static str = Box::leak(format!("home-row-{review_id}").into_boxed_str());
    let bounds = shell
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not painted"));
    f32::from(bounds.origin.y)
}

fn question(core: &Core, opened: &OpenedDiff) -> String {
    let blobs = BlobReader::open(&opened.repo).expect("blob reader");
    core.create_thread(
        &NewThread {
            review_id: opened.review_id.clone(),
            diff_id: opened.diff_id.clone(),
            subject: Subject::Review,
            kind: ThreadKind::Question,
            body_md: "Should `parse` reject duplicate keys?".into(),
            author: Author {
                kind: AuthorKind::Agent,
                name: "claude-code".into(),
                session_id: None,
            },
        },
        &blobs,
    )
    .expect("create the question")
}

#[gpui_kit::test]
fn home_sections_awaiting_you_and_recent(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let compare = core.open(&compare_req(repo.path())).unwrap();
    let head = core
        .open(&commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    let base = core
        .open(&commit_req(repo.path(), "refs/tags/base"))
        .unwrap();
    core.request_rereview(&compare.review_id, "Fixed the parser", &agent(), None)
        .unwrap();
    let now = now_ms();
    set_updated_at(&core, &compare.review_id, now - 3 * DAY);
    set_updated_at(&core, &head.review_id, now - HOUR);
    set_updated_at(&core, &base.review_id, now - 2 * DAY);

    let mut shell = start(cx);
    let (awaiting, recent) = sections(&mut shell);
    // Awaiting you first whatever its age; each section most recent first.
    assert_eq!(awaiting, std::slice::from_ref(&compare.review_id));
    assert_eq!(recent, [head.review_id.clone(), base.review_id.clone()]);

    // The row shows the repo, the title (branch or commit subject), kind,
    // status and relative time.
    let r = row_of(&mut shell, &compare.review_id);
    assert_eq!(r.title.as_ref(), "head");
    assert_eq!(r.source.as_ref(), "repo · base..head");
    assert_eq!(r.summary.kind, ReviewKind::Compare);
    assert_eq!(status_label(&r.summary).0, "Re-review requested");
    assert_eq!(
        row::viewed_label(&r.summary).as_deref(),
        Some("0 / 3 viewed")
    );
    let r = row_of(&mut shell, &head.review_id);
    assert_eq!(r.title.as_ref(), "head", "the commit's subject");
    assert!(r.source.starts_with("repo · "), "{}", r.source);
    assert_eq!(
        relative_time(now, r.summary.updated_at, 0),
        "1h ago",
        "last activity"
    );

    // Drawn in that order, sections included.
    let tops: Vec<f32> = [&compare, &head, &base]
        .iter()
        .map(|o| painted_top(&mut shell, &o.review_id))
        .collect();
    assert!(tops.windows(2).all(|w| w[0] < w[1]), "{tops:?}");
}

#[gpui_kit::test]
fn rereview_requested_review_is_awaiting_you(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    let mut shell = start(cx);
    assert_eq!(sections(&mut shell).0, Vec::<String>::new());

    core.submit_review(&opened.review_id, Verdict::RequestChanges, "", None)
        .unwrap();
    let r = {
        sections(&mut shell);
        row_of(&mut shell, &opened.review_id)
    };
    assert_eq!(status_label(&r.summary).0, "Changes requested");

    core.request_rereview(&opened.review_id, "Addressed everything", &agent(), None)
        .unwrap();
    assert_eq!(
        sections(&mut shell).0,
        std::slice::from_ref(&opened.review_id)
    );
    assert_eq!(
        status_label(&row_of(&mut shell, &opened.review_id).summary).0,
        "Re-review requested"
    );

    core.submit_review(&opened.review_id, Verdict::Approve, "LGTM", None)
        .unwrap();
    assert_eq!(
        sections(&mut shell),
        (vec![], vec![opened.review_id.clone()])
    );
    assert_eq!(
        status_label(&row_of(&mut shell, &opened.review_id).summary).0,
        "Approved"
    );
}

#[gpui_kit::test]
fn open_question_makes_review_awaiting_you(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    let mut shell = start(cx);
    assert!(sections(&mut shell).0.is_empty());

    let thread = question(&core, &opened);
    assert_eq!(
        sections(&mut shell).0,
        std::slice::from_ref(&opened.review_id)
    );
    let r = row_of(&mut shell, &opened.review_id);
    assert_eq!(
        row::questions_label(&r.summary).as_deref(),
        Some("1 question")
    );
    assert_eq!(
        row::threads_label(&r.summary).as_deref(),
        Some("1 open thread")
    );

    // Resolved, the question no longer waits for you.
    core.set_resolved(&thread, true, &agent(), None).unwrap();
    assert_eq!(
        sections(&mut shell),
        (vec![], vec![opened.review_id.clone()])
    );
}

#[gpui_kit::test]
fn archive_hides_review(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let kept = core
        .open(&commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    let archived = core.open(&compare_req(repo.path())).unwrap();
    let mut shell = start(cx);
    assert_eq!(sections(&mut shell).1.len(), 2);

    select(&mut shell, &archived.review_id);
    shell.cx.simulate_keystrokes("e");
    draw(shell.cx);
    assert_eq!(
        sections(&mut shell).1,
        std::slice::from_ref(&kept.review_id)
    );
    assert!(
        summary(&core, &archived.review_id).is_some(),
        "archived, not deleted"
    );
    assert!(home(&mut shell).read_with(shell.cx, |h, _| h.selected().is_none()));

    // Opening it again un-archives it.
    core.open(&compare_req(repo.path())).unwrap();
    assert_eq!(sections(&mut shell).1.len(), 2);
}

#[gpui_kit::test]
fn prune_confirms_then_deletes(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    let mut shell = start(cx);
    sections(&mut shell);
    let dialog_open =
        |shell: &mut Shell| shell.cx.update(|window, cx| window.has_active_dialog(cx));

    // ⌘⌫ asks first; Escape keeps the review.
    select(&mut shell, &opened.review_id);
    shell.cx.simulate_keystrokes("cmd-backspace");
    draw(shell.cx);
    assert!(dialog_open(&mut shell), "a confirmation, not a deletion");
    assert!(summary(&core, &opened.review_id).is_some());
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!dialog_open(&mut shell));
    assert!(summary(&core, &opened.review_id).is_some());
    assert_eq!(
        sections(&mut shell).1,
        std::slice::from_ref(&opened.review_id)
    );

    // Confirmed, it is gone for good.
    let home = home(&mut shell);
    home.update_in(shell.cx, |h, window, cx| {
        h.request_prune(&opened.review_id, window, cx)
    });
    draw(shell.cx);
    assert!(dialog_open(&mut shell));
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    assert!(!dialog_open(&mut shell));
    assert!(summary(&core, &opened.review_id).is_none());
    let (awaiting, recent) = sections(&mut shell);
    assert!(awaiting.is_empty() && recent.is_empty());
}

#[gpui_kit::test]
fn mute_toggles_review_muted(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    let mut shell = start(cx);
    sections(&mut shell);
    assert!(!row_of(&mut shell, &opened.review_id).summary.muted);

    // `j` selects the first row, `m` mutes it, `m` again unmutes it.
    shell.cx.simulate_keystrokes("j");
    draw(shell.cx);
    assert_eq!(
        home(&mut shell).read_with(shell.cx, |h, _| h.selected().map(str::to_owned)),
        Some(opened.review_id.clone())
    );
    shell.cx.simulate_keystrokes("m");
    draw(shell.cx);
    assert!(summary(&core, &opened.review_id).unwrap().muted);
    assert!(row_of(&mut shell, &opened.review_id).summary.muted);
    shell.cx.simulate_keystrokes("m");
    draw(shell.cx);
    assert!(!summary(&core, &opened.review_id).unwrap().muted);
    assert!(!row_of(&mut shell, &opened.review_id).summary.muted);
}

#[gpui_kit::test]
fn assign_to_session_reassigns_review(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    let session = |id: &str, client: &str| SessionInfo {
        id: id.into(),
        client_name: client.into(),
        client_version: None,
        owner_pid: None,
        cwd: Some(repo.path().to_path_buf()),
    };
    let first = core
        .upsert_session(&session("session-one", "claude-code"))
        .unwrap();
    let second = core
        .upsert_session(&session("session-two", "codex"))
        .unwrap();
    // Seen long ago: not offered.
    let stale = core
        .upsert_session(&session("session-old", "claude-code"))
        .unwrap();
    core.store
        .write(|tx| {
            tx.execute(
                &format!(
                    "UPDATE sessions SET last_seen_at = {} WHERE id = '{stale}'",
                    now_ms() - 8 * DAY
                ),
                (),
            )?;
            Ok(())
        })
        .unwrap();
    core.assign_review(&opened.review_id, &first, AssignedBy::OpenDiff)
        .unwrap();

    let mut shell = start(cx);
    sections(&mut shell);
    let r = row_of(&mut shell, &opened.review_id);
    assert_eq!(r.agent.as_deref(), Some("claude-code"), "agent badge");

    select(&mut shell, &opened.review_id);
    shell.cx.simulate_keystrokes("a");
    draw(shell.cx);
    assert!(shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    let offered = core.recent_sessions(now_ms() - 7 * DAY).unwrap();
    assert_eq!(offered.len(), 2, "the last 7 days only");
    assert!(shell.cx.debug_bounds("assign-session-2").is_none());
    let ix = offered.iter().position(|s| s.id == second).unwrap();
    let selector: &'static str = Box::leak(format!("assign-session-{ix}").into_boxed_str());
    let at = shell
        .cx
        .debug_bounds(selector)
        .expect("the session is listed")
        .center();
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);

    assert!(!shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(
        summary(&core, &opened.review_id)
            .unwrap()
            .assigned_session
            .as_deref(),
        Some(second.as_str())
    );
    let by: String = core
        .store
        .read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT assigned_by FROM review_assignments WHERE review_id = '{}'",
                    opened.review_id
                ),
                (),
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(by, "human");
    sections(&mut shell);
    assert_eq!(
        row_of(&mut shell, &opened.review_id).agent.as_deref(),
        Some("codex")
    );
}

#[gpui_kit::test]
fn auto_prune_runs_at_launch_only_when_setting_is_set(
    cx_off: &mut TestAppContext,
    cx_on: &mut TestAppContext,
) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let stale = core.open(&compare_req(repo.path())).unwrap();
    let fresh = core
        .open(&commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    set_updated_at(&core, &stale.review_id, now_ms() - 60 * DAY);

    // Off by default: nothing is pruned at launch.
    {
        let mut shell = start(cx_off);
        draw(shell.cx);
        assert_eq!(
            shell.cx.update(|_, cx| cx.global::<AutoPrune>().runs),
            0,
            "no run without the setting"
        );
        assert!(summary(&core, &stale.review_id).is_some());
        assert_eq!(sections(&mut shell).1.len(), 2);
    }

    // With `storage.prune_reviews_after_days`, the next launch prunes the
    // stale review and keeps the fresh one.
    let config = sb.config_dir().join("polygloss");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("settings.json"),
        r#"{ "storage": { "prune_reviews_after_days": 30 } }"#,
    )
    .unwrap();
    let mut shell = start(cx_on);
    draw(shell.cx);
    let (runs, pruned) = shell.cx.update(|_, cx| {
        let p = cx.global::<AutoPrune>();
        (p.runs, p.pruned.clone())
    });
    assert_eq!((runs, pruned), (1, vec![stale.review_id.clone()]));
    assert!(summary(&core, &stale.review_id).is_none());
    assert!(summary(&core, &fresh.review_id).is_some());
    let home = home(&mut shell);
    draw(shell.cx);
    let listed = home.read_with(shell.cx, |h, _| ids(h.rows()));
    assert_eq!(
        listed,
        std::slice::from_ref(&fresh.review_id),
        "Home refreshed"
    );

    // And again every 24 hours.
    set_updated_at(&core, &fresh.review_id, now_ms() - 45 * DAY);
    shell
        .cx
        .executor()
        .advance_clock(PRUNE_INTERVAL + Duration::from_secs(1));
    draw(shell.cx);
    assert_eq!(shell.cx.update(|_, cx| cx.global::<AutoPrune>().runs), 2);
    assert!(summary(&core, &fresh.review_id).is_none());
}

#[gpui_kit::test]
fn enter_opens_the_selected_review_and_focuses_its_tab(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    let mut shell = start(cx);
    sections(&mut shell);

    shell.cx.simulate_keystrokes("j enter");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1));
    let tab = shell.active_review().expect("a review tab");
    assert_eq!(
        tab.read_with(shell.cx, |t, _| t.review_id.clone()),
        opened.review_id
    );

    // Back on Home, clicking the row focuses the open tab.
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    let selector: &'static str =
        Box::leak(format!("home-row-{}", opened.review_id).into_boxed_str());
    let at = shell
        .cx
        .debug_bounds(selector)
        .expect("row painted")
        .center();
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1), "the existing tab, not a new one");
}

#[test]
fn review_keys_parse_back_into_open_requests() {
    let _sb = Sandbox::isolate();
    assert_eq!(
        parse_key("compare:refs/tags/v1.2.0...refs/heads/release-1.2"),
        Some(ParsedKey::Compare {
            base: "refs/tags/v1.2.0".into(),
            head: "refs/heads/release-1.2".into(),
            mode: CompareMode::ThreeDot,
        })
    );
    assert_eq!(
        parse_key("compare:refs/heads/main..refs/heads/spike"),
        Some(ParsedKey::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/spike".into(),
            mode: CompareMode::Direct,
        })
    );
    assert_eq!(
        parse_key("commit:9f1c2d3e"),
        Some(ParsedKey::Commit {
            oid: "9f1c2d3e".into()
        })
    );
    assert_eq!(
        parse_key("worktree:/src/app@feature/login#since=merge-base"),
        Some(ParsedKey::Live {
            worktree: "/src/app".into(),
            branch: "feature/login".into(),
            since: Since::MergeBase,
        })
    );
    // A worktree path holding `@` splits at the existing directory; a
    // branch holding `#` keeps it.
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().join("me@work");
    std::fs::create_dir_all(&wt).unwrap();
    let key = format!("worktree:{}@fix/#12@v2#since=HEAD", wt.display());
    assert_eq!(
        parse_key(&key),
        Some(ParsedKey::Live {
            worktree: wt.clone(),
            branch: "fix/#12@v2".into(),
            since: Since::Head,
        })
    );
    assert_eq!(
        parse_key("worktree:/src/app@main#since=0123abcd"),
        Some(ParsedKey::Live {
            worktree: "/src/app".into(),
            branch: "main".into(),
            since: Since::Commit("0123abcd".into()),
        })
    );
    for bad in ["", "pr:1", "compare:main", "commit:", "worktree:/x@main"] {
        assert_eq!(parse_key(bad), None, "{bad:?}");
    }

    // Open requests: live in its worktree, the others in the repo.
    let mut s = sample_summary("worktree:/src/app@main#since=merge-base", ReviewKind::Live);
    let req = open_request(&s).unwrap();
    assert_eq!(req.worktree, Path::new("/src/app"));
    assert_eq!(
        req.source,
        Source::Live {
            since: Since::MergeBase
        }
    );
    assert_eq!(req.label, None, "keeps the stored label");
    s.key = "compare:refs/heads/main...refs/heads/topic".into();
    let req = open_request(&s).unwrap();
    assert_eq!(req.worktree, Path::new("/repos/app"));
    assert_eq!(
        req.source,
        Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/topic".into(),
            mode: CompareMode::ThreeDot,
        }
    );
    s.key = "commit:0123456789abcdef".into();
    assert_eq!(
        open_request(&s).unwrap().source,
        Source::Commit {
            rev: "0123456789abcdef".into()
        }
    );
}

fn sample_summary(key: &str, kind: ReviewKind) -> ReviewSummary {
    ReviewSummary {
        review_id: "r1".into(),
        key: key.into(),
        label: None,
        kind,
        repo_display: "app".into(),
        repo_path: "/repos/app".into(),
        status: "open".into(),
        iterations: 1,
        latest_diff_id: None,
        viewed_done: 0,
        viewed_total: 0,
        open_threads: 0,
        open_questions: 0,
        awaiting_you: false,
        last_submission: None,
        rereview: None,
        assigned_session: None,
        muted: false,
        updated_at: 0,
    }
}

#[test]
fn rows_show_label_branch_or_subject_and_labels() {
    let _sb = Sandbox::isolate();
    let mut s = sample_summary(
        "compare:refs/remotes/origin/main...refs/heads/feature/login",
        ReviewKind::Compare,
    );
    let r = HomeRow::new(s.clone(), None, Some("claude-code"));
    assert_eq!(r.title.as_ref(), "feature/login");
    assert_eq!(r.source.as_ref(), "app · origin/main...feature/login");
    assert_eq!(r.agent.as_deref(), Some("claude-code"));
    s.label = Some("PR #42".into());
    assert_eq!(row::title(&s, None), "PR #42");

    s.key = "commit:0123456789abcdef0123456789abcdef01234567".into();
    s.label = None;
    assert_eq!(
        row::title(&s, Some("Parse config into a BTreeMap\n")),
        "Parse config into a BTreeMap"
    );
    assert_eq!(
        row::title(&s, None),
        "0123456",
        "short id until the subject is known"
    );
    assert_eq!(row::source_line(&s), "app · 0123456");

    s.key = "worktree:/src/app@topic#since=HEAD".into();
    assert_eq!(row::title(&s, None), "topic");
    assert_eq!(row::source_line(&s), "app · working tree since HEAD");
    s.label = Some("Spike".into());
    assert_eq!(
        row::source_line(&s),
        "app · topic · working tree since HEAD"
    );

    assert_eq!(status_label(&s).0, "Open");
    for (status, label) in [
        ("rereview_requested", "Re-review requested"),
        ("changes_requested", "Changes requested"),
        ("approved", "Approved"),
        ("commented", "Commented"),
    ] {
        s.status = status.into();
        assert_eq!(status_label(&s).0, label);
    }
    s.viewed_done = 2;
    s.viewed_total = 3;
    s.open_threads = 3;
    s.open_questions = 2;
    assert_eq!(row::viewed_label(&s).as_deref(), Some("2 / 3 viewed"));
    assert_eq!(row::threads_label(&s).as_deref(), Some("3 open threads"));
    assert_eq!(row::questions_label(&s).as_deref(), Some("2 questions"));
    assert_eq!(row::kind_label(ReviewKind::Live), "LIVE");
}

#[test]
fn relative_times_read_like_github() {
    // 2026-09-29 12:00:00 UTC.
    let now = 1_790_683_200_000;
    let min = 60_000;
    assert_eq!(relative_time(now, now + 5 * min, 0), "just now");
    assert_eq!(relative_time(now, now - 30_000, 0), "just now");
    assert_eq!(relative_time(now, now - 5 * min, 0), "5m ago");
    assert_eq!(relative_time(now, now - 3 * HOUR, 0), "3h ago");
    assert_eq!(relative_time(now, now - 30 * HOUR, 0), "yesterday");
    assert_eq!(relative_time(now, now - 4 * DAY, 0), "4d ago");
    assert_eq!(relative_time(now, now - 26 * DAY, 0), "Sep 3");
    assert_eq!(relative_time(now, now - 400 * DAY, 0), "Aug 25, 2025");
    // The date is local: 23:30 UTC on Sep 3 is Sep 4 two hours east.
    let late = 1_788_478_200_000; // 2026-09-03 23:30 UTC
    assert_eq!(relative_time(now, late, 0), "Sep 3");
    assert_eq!(relative_time(now, late, 2 * 3600), "Sep 4");
    assert_eq!(relative_time(now, 0, 0), "Jan 1, 1970");
}
