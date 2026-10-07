//! Home and recents (T3.4, design §11.2, §17, OQ-32, OQ-34): the two
//! sections, what a row shows, the row actions (open, archive, prune, mute,
//! assign to session) and the automatic prune at launch. Also the sidebar's
//! Reviews segment (T6.6, OQ-36): Home, the open reviews, and with a review
//! active Awaiting you and Recent with Home's row menu.

use std::path::Path;
use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use polygloss_app::home::HomeView;
use polygloss_app::home::prune::{AutoPrune, PRUNE_INTERVAL};
use polygloss_app::home::row::{
    self, HomeRow, ParsedKey, open_request, parse_key, relative_time, status_label,
};
use polygloss_app::motion::{self, MotionPolicy};
use polygloss_app::tabs::TabItem;
use polygloss_core::git::{CompareMode, ReviewKind, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::threads::{Author, AuthorKind, NewThread, Subject, ThreadKind};
use polygloss_core::review::{
    AssignedBy, Core, OpenRequest, OpenedDiff, ReviewFilter, ReviewSummary, SessionInfo, Verdict,
};
use polygloss_core::store::events::{Actor, ActorKind, now_ms};

use crate::shell::{Shell, bounds, click, compare_req, draw, hover, painted, start, unhover};
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

    // Motion Off: gpui-kit's dialog slides in for 250 ms on the wall clock,
    // so on a loaded machine the click below would land on a moved button.
    shell
        .cx
        .update(|_, cx| motion::set_override(Some(MotionPolicy::Off), cx));
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

/// `review_id`'s live open: the code-change fixture's working tree since
/// HEAD.
fn live_req(repo: &Path) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Live { since: Since::Head },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// Three reviews of `repo`: a compare awaiting you (re-review requested),
/// then two commits under Recent, `head` more recent than `base`.
fn three_reviews(core: &Core, repo: &Path) -> (OpenedDiff, OpenedDiff, OpenedDiff) {
    let compare = core.open(&compare_req(repo)).unwrap();
    let head = core.open(&commit_req(repo, "refs/tags/head")).unwrap();
    let base = core.open(&commit_req(repo, "refs/tags/base")).unwrap();
    core.request_rereview(&compare.review_id, "Fixed the parser", &agent(), None)
        .unwrap();
    let now = now_ms();
    set_updated_at(core, &compare.review_id, now - 3 * DAY);
    set_updated_at(core, &head.review_id, now - HOUR);
    set_updated_at(core, &base.review_id, now - 2 * DAY);
    (compare, head, base)
}

/// The tops of `selectors`, asserting each one is painted below the
/// previous one.
fn assert_stacked(shell: &mut Shell, selectors: &[String]) {
    let mut last = None;
    for name in selectors {
        let top = bounds(shell.cx, name).top();
        if let Some((prev_name, prev)) = last {
            assert!(top > prev, "{name} is below {prev_name}");
        }
        last = Some((name.clone(), top));
    }
}

#[gpui_kit::test]
fn reviews_segment_lists_home_open_awaiting_and_recent(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let (compare, head, base) = three_reviews(&core, repo.path());
    let mut shell = start(cx);
    sections(&mut shell);
    shell.open(compare_req(repo.path())).unwrap();
    std::fs::write(repo.path().join("NOTES.md"), "live\n").unwrap();
    let live = shell.open(live_req(repo.path())).unwrap();
    let live_id = live.read_with(shell.cx, |t, _| t.review_id.clone());
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    sections(&mut shell);
    click(shell.cx, "segment-reviews");

    let open = |id: &str| format!("open-review-{id}");
    let row = |id: &str| format!("nav-row-{id}");
    // Home (one review awaits you), Open with its reviews in tab order,
    // Awaiting you, then Recent, most recent first.
    assert_stacked(
        &mut shell,
        &[
            "nav-home".into(),
            "nav-section-open".into(),
            open(&compare.review_id),
            open(&live_id),
            open(&head.review_id),
            "nav-section-awaiting".into(),
            row(&compare.review_id),
            "nav-section-recent".into(),
            row(&head.review_id),
            row(&base.review_id),
        ],
    );
    assert!(painted(shell.cx, "nav-home-awaiting-1").is_some());
    // Open… sits in the Open heading.
    let heading = bounds(shell.cx, "nav-section-open");
    let open_button = bounds(shell.cx, "nav-open");
    assert!(heading.contains(&open_button.center()));
    // Only the live review has the live dot.
    assert!(painted(shell.cx, &format!("nav-live-dot-{live_id}")).is_some());
    for id in [&compare.review_id, &head.review_id] {
        assert!(painted(shell.cx, &format!("nav-live-dot-{id}")).is_none());
    }
    // Awaiting you and Recent list Home's sections, row for row.
    let home = home(&mut shell);
    let (awaiting, recent) =
        home.read_with(shell.cx, |h, _| (ids(h.awaiting_you()), ids(h.recent())));
    assert_eq!(awaiting, std::slice::from_ref(&compare.review_id));
    assert!(recent.contains(&live_id), "the live review is recent too");
    for id in awaiting.iter().chain(&recent) {
        assert!(painted(shell.cx, &row(id)).is_some(), "{id}");
    }
}

#[gpui_kit::test]
fn reviews_segment_on_home_lists_only_home_and_open(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let (compare, head, base) = three_reviews(&core, repo.path());
    let mut shell = start(cx);
    sections(&mut shell);
    // No review open: Home, and Open with its button only.
    assert!(painted(shell.cx, "nav-home").is_some());
    assert!(painted(shell.cx, "nav-section-open").is_some());
    assert!(painted(shell.cx, "nav-open").is_some());
    assert!(painted(shell.cx, "nav-home-awaiting-1").is_some());
    // One open, Home showing: Home and Open, never the cards' lists.
    shell.open(compare_req(repo.path())).unwrap();
    shell.cx.simulate_keystrokes("cmd-0");
    sections(&mut shell);
    assert_eq!(shell.tabs(), (2, 0));
    assert!(painted(shell.cx, &format!("open-review-{}", compare.review_id)).is_some());
    for name in ["nav-section-awaiting", "nav-section-recent"] {
        assert!(painted(shell.cx, name).is_none(), "{name}");
    }
    for o in [&compare, &head, &base] {
        assert!(painted(shell.cx, &format!("nav-row-{}", o.review_id)).is_none());
    }
    // With the review active again, both lists come back.
    shell.cx.simulate_keystrokes("cmd-1");
    draw(shell.cx);
    click(shell.cx, "segment-reviews");
    assert!(painted(shell.cx, "nav-section-recent").is_some());
}

#[gpui_kit::test]
fn nav_row_click_opens_or_focuses(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let (compare, head, _) = three_reviews(&core, repo.path());
    let mut shell = start(cx);
    sections(&mut shell);
    shell.open(compare_req(repo.path())).unwrap();
    click(shell.cx, "segment-reviews");

    // A Recent row of a review not open opens it (a first open, so the
    // sidebar shows its files).
    click(shell.cx, &format!("nav-row-{}", head.review_id));
    assert_eq!(shell.tabs(), (3, 2));
    let active = shell.active_review().expect("a review");
    assert_eq!(
        active.read_with(shell.cx, |t, _| t.review_id.clone()),
        head.review_id
    );
    assert!(painted(shell.cx, "file-tree-pane").is_some());
    // An Awaiting you row of an open review focuses its tab.
    click(shell.cx, "segment-reviews");
    click(shell.cx, &format!("nav-row-{}", compare.review_id));
    assert_eq!(shell.tabs(), (3, 1), "the existing tab, not a new one");
    assert!(painted(shell.cx, "nav").is_some(), "the segment stays");
    // An Open row focuses its review; Home's row shows Home.
    click(shell.cx, &format!("open-review-{}", head.review_id));
    assert_eq!(shell.tabs(), (3, 2));
    click(shell.cx, "nav-home");
    assert_eq!(shell.tabs(), (3, 0));
}

#[gpui_kit::test]
fn nav_open_button_opens_the_open_flow(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let flow_open = |shell: &mut Shell| {
        shell.cx.update(|window, cx| {
            polygloss_app::open_flow::current(cx).is_some() && window.has_active_dialog(cx)
        })
    };
    // On Home.
    click(shell.cx, "nav-open");
    assert!(flow_open(&mut shell));
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!flow_open(&mut shell));
    // In a review, from the Reviews segment.
    shell.open(compare_req(repo.path())).unwrap();
    click(shell.cx, "segment-reviews");
    click(shell.cx, "nav-open");
    assert!(flow_open(&mut shell));
}

#[gpui_kit::test]
fn home_toolbar_open_button_opens_the_open_flow(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    // Design §11.2: Home's toolbar row holds "Reviews", the count and
    // Open… (⌘O); the button sits in the row's right half.
    let toolbar = bounds(shell.cx, "home-toolbar");
    let open = bounds(shell.cx, "home-open");
    assert!(toolbar.contains(&open.center()), "{open:?} in {toolbar:?}");
    assert!(
        open.center().x > toolbar.center().x,
        "{open:?} on the right"
    );
    click(shell.cx, "home-open");
    assert!(shell.cx.update(|window, cx| {
        polygloss_app::open_flow::current(cx).is_some() && window.has_active_dialog(cx)
    }));
}

/// Opens `review_id`'s ⋯ menu in the Reviews segment (it shows on hover).
fn open_nav_row_menu(shell: &mut Shell, review_id: &str) {
    hover(shell.cx, &format!("nav-row-{review_id}"));
    click(shell.cx, &format!("nav-row-menu-{review_id}"));
}

#[gpui_kit::test]
fn nav_row_menu_archives_mutes_and_assigns(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let (compare, head, base) = three_reviews(&core, repo.path());
    core.upsert_session(&SessionInfo {
        id: "session-one".into(),
        client_name: "claude-code".into(),
        client_version: None,
        owner_pid: None,
        cwd: Some(repo.path().to_path_buf()),
    })
    .unwrap();
    let mut shell = start(cx);
    sections(&mut shell);
    shell.open(compare_req(repo.path())).unwrap();
    click(shell.cx, "segment-reviews");
    let has_dialog = |shell: &mut Shell| shell.cx.update(|window, cx| window.has_active_dialog(cx));

    // Home's menu: Open, Mute, Assign to session…, Archive, Prune…. Its ⋯
    // stays while the menu is open, wherever the pointer goes, and hides
    // with it.
    let menu_button = format!("nav-row-menu-{}", head.review_id);
    open_nav_row_menu(&mut shell, &head.review_id);
    unhover(shell.cx);
    assert!(painted(shell.cx, &menu_button).is_some(), "kept while open");
    shell.cx.simulate_keystrokes("down down enter");
    draw(shell.cx);
    assert!(summary(&core, &head.review_id).unwrap().muted, "muted");
    assert_eq!(shell.tabs(), (2, 1), "nothing opened");
    assert!(painted(shell.cx, &menu_button).is_none(), "hidden again");

    // The context menu is the same menu: archive.
    let at = bounds(shell.cx, &format!("nav-row-{}", base.review_id)).center();
    shell.cx.simulate_mouse_down(
        at,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::none(),
    );
    shell.cx.simulate_mouse_up(
        at,
        gpui_kit::MouseButton::Right,
        gpui_kit::Modifiers::none(),
    );
    draw(shell.cx);
    shell.cx.simulate_keystrokes("down down down down enter");
    draw(shell.cx);
    sections(&mut shell);
    let archived = core
        .review_summaries(&ReviewFilter::default())
        .unwrap()
        .into_iter()
        .all(|s| s.review_id != base.review_id);
    assert!(archived, "archived: hidden from the list");
    assert!(summary(&core, &base.review_id).is_some(), "not deleted");
    assert!(painted(shell.cx, &format!("nav-row-{}", base.review_id)).is_none());

    // Assign to session… opens the picker.
    open_nav_row_menu(&mut shell, &compare.review_id);
    shell.cx.simulate_keystrokes("down down down enter");
    draw(shell.cx);
    assert!(has_dialog(&mut shell), "the session picker");
    assert!(painted(shell.cx, "assign-session-0").is_some());
}

/// ADR-0031 (T7.6): Home's cards sit as far apart as the review canvas's,
/// its sections `SECTION` apart, and its badges (kind, status, questions,
/// agent) are one shape.
#[gpui_kit::test]
fn home_cards_share_the_canvas_gap(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let core = Core::open_default().unwrap();
    let (compare, head, base) = three_reviews(&core, repo.path());
    question(&core, &compare);
    let session = core
        .upsert_session(&SessionInfo {
            id: "session-one".into(),
            client_name: "claude-code".into(),
            client_version: None,
            owner_pid: None,
            cwd: Some(repo.path().to_path_buf()),
        })
        .unwrap();
    core.assign_review(&head.review_id, &session, AssignedBy::OpenDiff)
        .unwrap();
    let mut shell = start(cx);
    assert_eq!(
        sections(&mut shell),
        (
            vec![compare.review_id.clone()],
            vec![head.review_id.clone(), base.review_id.clone()]
        )
    );

    // Two cards of one section (Recent): the space between them.
    let upper = bounds(shell.cx, &format!("home-row-{}", head.review_id));
    let lower = bounds(shell.cx, &format!("home-row-{}", base.review_id));
    let home_gap = lower.top() - upper.bottom();
    // Between sections: Awaiting you's last card to Recent's title, 24.
    let awaiting = bounds(shell.cx, &format!("home-row-{}", compare.review_id));
    let recent = bounds(shell.cx, "home-section-RECENT");
    assert_eq!(recent.top() - awaiting.bottom(), gpui_kit::px(24.));
    // Every badge is 20 tall, ...
    let mut badges = vec![
        format!("home-questions-{}", compare.review_id),
        format!("home-agent-{}", head.review_id),
    ];
    for id in [&compare.review_id, &head.review_id, &base.review_id] {
        badges.push(format!("home-kind-{id}"));
        badges.push(format!("home-status-{id}"));
    }
    for name in &badges {
        assert_eq!(
            bounds(shell.cx, name).size.height,
            gpui_kit::px(20.),
            "{name}"
        );
    }
    // ... and the one badge they are built from is a capsule: radius 10.
    let mut badge = shell
        .cx
        .update(|_, cx| row::badge(gpui_kit::component::ActiveTheme::theme(cx).foreground));
    let radii = badge.style().corner_radii.clone();
    for corner in [
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    ] {
        assert_eq!(corner, Some(gpui_kit::px(10.).into()));
    }

    // The review canvas, measured the same way: the header card's bottom to
    // the first file card's top (its header's top edge).
    let tab = shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    let card = bounds(shell.cx, "header-card");
    let viewport = bounds(shell.cx, "viewport-pane");
    let first = tab.read_with(shell.cx, |t, cx| {
        let headers = t.viewport.read(cx).debug().headers;
        headers.first().expect("a file card is painted").y
    });
    let canvas_gap = viewport.top() + gpui_kit::px(first) - card.bottom();
    assert!(
        canvas_gap > gpui_kit::px(0.),
        "the canvas has a gap: {canvas_gap:?}"
    );
    assert_eq!(home_gap, canvas_gap);
}
