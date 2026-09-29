//! GPUI tests of T3.13: the store feed in the app (design §7.1 "Change
//! notification", §8.3 step 5, §11.7, §17). `StoreFeed` polls the store's
//! `EventFeed` (150 ms focused, 1 s unfocused, or at once on a nudge) and
//! hands new events to the open tabs and Home: agent activity since
//! `reviews.last_seen_seq` shows "claude-code replied to N threads" (the
//! button jumps to the next unread thread and marks it seen), a re-review
//! request shows its summary with "View changes", and threads reload so
//! only the changed blocks are touched.
//!
//! Writes that the feed must notice come from another connection (a second
//! `Core`, as the CLI or `polygloss mcp` would open), and in
//! `agent_reply_from_other_process_shows_banner` from a real child process
//! (this test binary re-run as a writer, `child_writes_an_agent_reply`).

use std::process::{Command, Stdio};
use std::time::Duration;

use gpui_kit::{Entity, SharedString};
use polygloss_app::feed::{self, FOCUSED_INTERVAL, StoreFeed, UNFOCUSED_INTERVAL};
use polygloss_app::home::HomeView;
use polygloss_app::keymap::actions::tab as tab_actions;
use polygloss_app::review_tab::{BannerKind, ReviewTab};
use polygloss_app::tabs::TabItem;
use polygloss_app::threads::{self, ReviewThreads, placement};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, Subject, ThreadKind, Verdict, Viewer,
};
use polygloss_core::store::events::{Actor, ActorKind};
use polygloss_diff::Side;

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

fn agent() -> Author {
    Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
    }
}

fn agent_actor() -> Actor {
    Actor {
        kind: ActorKind::Agent,
        name: Some("claude-code".into()),
        session_id: None,
    }
}

/// Another connection to the sandbox store, like the CLI or `polygloss mcp`.
fn other_core() -> Core {
    Core::open_default().expect("open a second connection to the store")
}

fn line(path: &str, line: u32) -> Subject {
    Subject::Line {
        path: path.into(),
        side: Side::New,
        start_line: line,
        line,
    }
}

/// An agent thread on the diff of `tab`, written through `core`.
fn agent_thread(
    core: &Core,
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
    subject: Subject,
    body: &str,
) -> String {
    let (review_id, diff_id, repo) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo).expect("open the object store");
    core.create_thread(
        &NewThread {
            review_id,
            diff_id,
            subject,
            kind: ThreadKind::Comment,
            body_md: body.into(),
            author: agent(),
        },
        &blobs,
    )
    .expect("create the agent thread")
}

fn review_id(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, _| t.review_id.clone())
}

/// Wakes the feed at once (the socket's `store_changed` in M4) and lets
/// everything it starts finish.
fn nudge(shell: &mut Shell) {
    shell.cx.update(|_, cx| feed::nudge(cx));
    draw(shell.cx);
}

/// Advances GPUI's fake clock in `step`s, letting the feed poll.
fn advance(shell: &mut Shell, total: Duration, step: Duration) {
    let mut left = total;
    while !left.is_zero() {
        let d = step.min(left);
        shell.cx.executor().advance_clock(d);
        shell.cx.run_until_parked();
        left -= d;
    }
    draw(shell.cx);
}

fn banners(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<(BannerKind, SharedString)> {
    tab.read_with(shell.cx, |t, cx| t.banners.read(cx).banners())
}

fn banner(shell: &mut Shell, tab: &Entity<ReviewTab>, kind: BannerKind) -> Option<String> {
    banners(shell, tab)
        .into_iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, text)| text.to_string())
}

fn model(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Entity<ReviewThreads> {
    tab.read_with(shell.cx, |t, _| {
        threads::threads(t).cloned().expect("threads are attached")
    })
}

fn last_seen(core: &Core, review_id: &str) -> i64 {
    core.review_activity(review_id).unwrap().last_seen_seq
}

/// Clicks the banner button of `kind` (the strip lists banners in
/// `BannerKind` order), with nothing focused.
fn click_banner(shell: &mut Shell, tab: &Entity<ReviewTab>, kind: BannerKind) {
    let ix = banners(shell, tab)
        .iter()
        .position(|(k, _)| *k == kind)
        .unwrap_or_else(|| panic!("no {kind:?} banner"));
    let name: &'static str = Box::leak(format!("banner-button-{ix}").into_boxed_str());
    shell.cx.update(|window, cx| window.blur(cx));
    let at = shell
        .cx
        .debug_bounds(name)
        .unwrap_or_else(|| panic!("{name} is not painted"))
        .center();
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);
}

// ---------------------------------------------------------------------------
// Another process

const CHILD_ENV: &str = "POLYGLOSS_FEED_CHILD_THREAD";

/// The writer child of `agent_reply_from_other_process_shows_banner`: replies
/// as claude-code to the thread named by `POLYGLOSS_FEED_CHILD_THREAD`. A
/// no-op in a normal run.
#[test]
fn child_writes_an_agent_reply() {
    let Some(thread_id) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let core = Core::open_default().expect("open the store in the child");
    let id = core
        .reply(
            thread_id.to_str().unwrap(),
            "Fixed in the next commit.",
            &agent(),
        )
        .expect("reply as the agent");
    println!("CHILD-COMMENT {id}");
}

#[gpui_kit::test]
fn agent_reply_from_other_process_shows_banner(cx: &mut gpui_kit::TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    // A published human thread the agent can see.
    let thread = {
        let blobs = BlobReader::open(&repo_info(&mut shell, &tab)).unwrap();
        let (review_id, diff_id) = tab.read_with(shell.cx, |t, _| {
            (t.review_id.clone(), t.opened.diff_id.clone())
        });
        let id = shell
            .core
            .create_thread(
                &NewThread {
                    review_id,
                    diff_id,
                    subject: line("src/config.rs", 5),
                    kind: ThreadKind::Comment,
                    body_md: "BTreeMap?".into(),
                    author: Author {
                        kind: AuthorKind::Human,
                        name: "you".into(),
                        session_id: None,
                    },
                },
                &blobs,
            )
            .unwrap();
        shell
            .core
            .submit_review(&review, Verdict::RequestChanges, "", None)
            .unwrap();
        id
    };
    shell.cx.update(|window, _| window.activate_window());
    advance(&mut shell, Duration::from_millis(300), FOCUSED_INTERVAL);
    assert_eq!(banner(&mut shell, &tab, BannerKind::AgentReplies), None);

    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "feed::child_writes_an_agent_reply",
            "--nocapture",
        ])
        .env(CHILD_ENV, &thread)
        .env("POLYGLOSS_DATA_DIR", sb.data_dir())
        .env("HOME", sb.home())
        .env("XDG_CONFIG_HOME", sb.config_dir())
        .env("XDG_CACHE_HOME", sb.cache_dir())
        .env("GIT_CONFIG_GLOBAL", sb.git_config_global())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "child failed: {}\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let comment = stdout
        .lines()
        .find_map(|l| l.strip_prefix("CHILD-COMMENT "))
        .unwrap_or_else(|| panic!("no CHILD-COMMENT line in:\n{stdout}"))
        .trim()
        .to_owned();

    // No nudge: the next focused poll (150 ms) sees it.
    advance(&mut shell, FOCUSED_INTERVAL, FOCUSED_INTERVAL);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 1 thread")
    );
    // The thread reloaded with the reply.
    let m = model(&mut shell, &tab);
    let comments = m.read_with(shell.cx, |m, _| {
        m.thread(&thread)
            .map(|t| t.comments.iter().map(|c| c.id.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[1], comment);
    // Nothing was marked seen by showing the banner.
    assert_eq!(last_seen(&shell.core, &review), 0);
}

fn repo_info(shell: &mut Shell, tab: &Entity<ReviewTab>) -> polygloss_core::git::RepoInfo {
    tab.read_with(shell.cx, |t, _| t.opened.repo.clone())
}

#[gpui_kit::test]
fn banner_count_uses_last_seen_seq(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    let other = other_core();
    let a = agent_thread(&other, &mut shell, &tab, line("src/config.rs", 1), "a");
    let b = agent_thread(&other, &mut shell, &tab, line("src/config.rs", 5), "b");
    let c = agent_thread(&other, &mut shell, &tab, line("src/greet.ts", 3), "c");
    nudge(&mut shell);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 3 threads")
    );

    // Seen up to `b`: one thread left, in this tab and in a tab opened now.
    let seq_b = other
        .review_activity(&review)
        .unwrap()
        .unread
        .iter()
        .find(|u| u.thread_id == b)
        .unwrap()
        .last_seq;
    other.mark_seen(&review, seq_b).unwrap();
    // `last_seen_seq` has no event of its own (design §7.3): a tab reads it
    // when it opens and whenever agent activity comes in.
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 1 thread")
    );
    let unread = tab.read_with(shell.cx, |t, _| {
        feed::activity(t)
            .map(|a| {
                a.unread
                    .iter()
                    .map(|u| u.thread_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    assert_eq!(unread, [c]);

    // A reply on `a` (seen before) counts again; a second reply on the same
    // thread does not add a thread.
    other.reply(&a, "one", &agent()).unwrap();
    nudge(&mut shell);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 2 threads")
    );
    other.reply(&a, "two", &agent()).unwrap();
    other.set_resolved(&a, true, &agent_actor(), None).unwrap();
    nudge(&mut shell);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 2 threads")
    );

    // A human's own activity never counts.
    shell
        .core
        .set_resolved(&a, false, &Actor::human(), None)
        .unwrap();
    nudge(&mut shell);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 2 threads")
    );
}

#[gpui_kit::test]
fn jump_to_next_unread_marks_seen(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    let other = other_core();
    // Visited in the order the agent last touched them: `second` first
    // (created earlier, never touched again), then `first` (replied to).
    let first = agent_thread(&other, &mut shell, &tab, line("src/config.rs", 5), "first");
    let second = agent_thread(&other, &mut shell, &tab, line("src/greet.ts", 3), "second");
    other.reply(&first, "and a reply", &agent()).unwrap();
    nudge(&mut shell);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 2 threads")
    );
    let unread = other.review_activity(&review).unwrap().unread;
    assert_eq!(unread[0].thread_id, second);
    assert_eq!(unread[1].thread_id, first);

    // "Show": the cursor on `second`'s line (greet.ts, new line 3), seen up
    // to its last event.
    click_banner(&mut shell, &tab, BannerKind::AgentReplies);
    let cursor = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor().unwrap());
    let greet = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .position(|f| f.display_path() == "src/greet.ts")
            .unwrap() as u32
    });
    assert_eq!(
        (cursor.file_idx, cursor.side, cursor.line),
        (greet, Side::New, 2)
    );
    assert_eq!(last_seen(&shell.core, &review), unread[0].last_seq);
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 1 thread")
    );

    // Again (the palette action this time): `first`, and nothing is left.
    shell.cx.update(|window, cx| {
        let focus = tab.read(cx).viewport_focus().clone();
        focus.dispatch_action(&tab_actions::NextUnreadThread, window, cx);
    });
    draw(shell.cx);
    let cursor = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor().unwrap());
    assert_eq!((cursor.file_idx, cursor.line), (0, 4));
    assert_eq!(last_seen(&shell.core, &review), unread[1].last_seq);
    assert_eq!(banner(&mut shell, &tab, BannerKind::AgentReplies), None);
    // The viewport has the keyboard.
    let focused = shell
        .cx
        .update(|window, cx| tab.read(cx).viewport_focus().is_focused(window));
    assert!(focused);
}

#[gpui_kit::test]
fn rereview_banner_offers_changes_since(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    let other = other_core();
    other
        .request_rereview(
            &review,
            "Switched to **BTreeMap** as asked.\n\nAlso added `is_empty`.",
            &agent_actor(),
            None,
        )
        .unwrap();
    nudge(&mut shell);
    // The summary's first line, as plain text.
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::Rereview).as_deref(),
        Some("claude-code requested a re-review: Switched to BTreeMap as asked.")
    );
    // Its button is "View changes": Changes since last review (T3.12).
    let action = tab.read_with(shell.cx, |t, cx| {
        t.banners
            .read(cx)
            .action(BannerKind::Rereview)
            .map(|a| a.name().to_owned())
    });
    assert_eq!(action.as_deref(), Some("tab::ToggleChangesSinceLastReview"));
    assert!(shell.cx.debug_bounds("banner-button-0").is_some());
    // A re-review request is no agent reply.
    assert_eq!(banner(&mut shell, &tab, BannerKind::AgentReplies), None);

    // A tab opened later shows it too (read when the tab opens).
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert!(banner(&mut shell, &tab, BannerKind::Rereview).is_some());

    // Submitting ends the request.
    other
        .submit_review(&review, Verdict::Approve, "", None)
        .unwrap();
    nudge(&mut shell);
    assert_eq!(banner(&mut shell, &tab, BannerKind::Rereview), None);
}

#[gpui_kit::test]
fn agent_event_invalidates_only_that_files_blocks(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let other = other_core();
    let on_config = agent_thread(&other, &mut shell, &tab, line("src/config.rs", 5), "c");
    let on_greet = agent_thread(&other, &mut shell, &tab, line("src/greet.ts", 3), "g");
    nudge(&mut shell);
    let m = model(&mut shell, &tab);
    assert!(m.read_with(shell.cx, |m, _| m.thread(&on_greet).is_some()));
    let block_sets = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            let doc = t.viewport.read(cx).document();
            (doc.block_sets(0), doc.block_sets(1), doc.block_sets(2))
        })
    };
    let before = block_sets(&mut shell);
    m.update(shell.cx, |m, _| m.reset_stats());

    // An agent reply on the greet.ts thread: one block re-measured, no file
    // re-laid out.
    other.reply(&on_greet, "done", &agent()).unwrap();
    nudge(&mut shell);
    let stats = m.read_with(shell.cx, |m, _| m.stats().clone());
    assert_eq!(stats.loads, 1);
    assert!(stats.set_blocks.is_empty(), "{stats:?}");
    assert_eq!(stats.invalidated, [placement::block_id(&on_greet)]);
    assert_eq!(block_sets(&mut shell), before);
    let _ = on_config;

    // A new agent thread on config.rs: that file's blocks only.
    m.update(shell.cx, |m, _| m.reset_stats());
    agent_thread(&other, &mut shell, &tab, line("src/config.rs", 1), "new");
    nudge(&mut shell);
    let stats = m.read_with(shell.cx, |m, _| m.stats().clone());
    assert_eq!(stats.set_blocks.keys().copied().collect::<Vec<_>>(), [0]);
    assert!(stats.invalidated.is_empty(), "{stats:?}");
    let after = block_sets(&mut shell);
    assert_eq!((after.1, after.2), (before.1, before.2));

    // Events of another review, on another diff, reload nothing here.
    m.update(shell.cx, |m, _| m.reset_stats());
    let other_tab = shell
        .open(polygloss_core::review::OpenRequest {
            source: polygloss_core::git::Source::Commit {
                rev: "refs/tags/base".into(),
            },
            ..compare_req(repo.path())
        })
        .unwrap();
    agent_thread(
        &other,
        &mut shell,
        &other_tab,
        line("src/config.rs", 1),
        "x",
    );
    nudge(&mut shell);
    assert_eq!(m.read_with(shell.cx, |m, _| m.stats().loads), 0);
    assert_eq!(
        banner(&mut shell, &other_tab, BannerKind::AgentReplies).as_deref(),
        Some("claude-code replied to 1 thread")
    );
}

#[gpui_kit::test]
fn feed_poll_interval_slows_when_unfocused(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let polls = |shell: &mut Shell| shell.cx.update(|_, cx| StoreFeed::global(cx).stats().polls);
    let interval = |shell: &mut Shell| shell.cx.update(|_, cx| StoreFeed::global(cx).interval());

    // Focused: a poll every 150 ms.
    shell.cx.update(|window, _| window.activate_window());
    draw(shell.cx);
    assert_eq!(interval(&mut shell), FOCUSED_INTERVAL);
    let start = polls(&mut shell);
    advance(
        &mut shell,
        Duration::from_millis(1500),
        Duration::from_millis(50),
    );
    let focused = polls(&mut shell) - start;
    assert!(
        (9..=11).contains(&focused),
        "{focused} polls in 1.5 s focused"
    );

    // Unfocused: one a second.
    shell.cx.deactivate_window();
    draw(shell.cx);
    let start = polls(&mut shell);
    advance(
        &mut shell,
        Duration::from_millis(3000),
        Duration::from_millis(50),
    );
    let unfocused = polls(&mut shell) - start;
    assert!(
        (2..=4).contains(&unfocused),
        "{unfocused} polls in 3 s unfocused"
    );
    assert_eq!(interval(&mut shell), UNFOCUSED_INTERVAL);

    // Focused again: a poll at once, then the fast rate.
    let start = polls(&mut shell);
    shell.cx.update(|window, _| window.activate_window());
    draw(shell.cx);
    assert!(polls(&mut shell) > start, "activation polls at once");
    assert_eq!(interval(&mut shell), FOCUSED_INTERVAL);

    // A nudge polls at once, whatever the interval.
    let start = polls(&mut shell);
    nudge(&mut shell);
    assert_eq!(polls(&mut shell), start + 1);
}

#[gpui_kit::test]
fn store_events_refresh_home(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    let home: Entity<HomeView> = match shell.tab(0) {
        TabItem::Home(home) => home,
        TabItem::Review(_) => panic!("Home is the first tab"),
    };
    let awaiting = |shell: &mut Shell| {
        home.read_with(shell.cx, |h, _| {
            h.awaiting_you()
                .iter()
                .map(|r| r.review_id().to_owned())
                .collect::<Vec<_>>()
        })
    };
    assert!(awaiting(&mut shell).is_empty());
    other_core()
        .request_rereview(&review, "Please look again.", &agent_actor(), None)
        .unwrap();
    nudge(&mut shell);
    assert_eq!(awaiting(&mut shell), [review]);
}

#[gpui_kit::test]
fn next_unread_opens_a_thread_the_diff_cannot_show_in_the_panel(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let other = other_core();
    let question = agent_thread(&other, &mut shell, &tab, Subject::Review, "Ship it?");
    nudge(&mut shell);
    tab.update(shell.cx, |t, cx| {
        if t.threads_panel_visible() {
            t.toggle_threads_panel(cx);
        }
    });
    draw(shell.cx);
    click_banner(&mut shell, &tab, BannerKind::AgentReplies);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    let m = model(&mut shell, &tab);
    assert!(m.read_with(shell.cx, |m, _| m.is_expanded(&question)));
    assert_eq!(banner(&mut shell, &tab, BannerKind::AgentReplies), None);
    // The thread is still there for the human.
    assert!(
        shell
            .core
            .thread(&question, Viewer::Human)
            .is_ok_and(|t| t.comments.len() == 1)
    );
}

#[gpui_kit::test]
fn long_rereview_summary_keeps_its_button_in_view(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    let other = other_core();
    let a = agent_thread(&other, &mut shell, &tab, line("src/config.rs", 1), "a");
    let _ = a;
    let summary = "Reworked the config loader so that every key is validated, sorted and \
                   documented, and the error messages now name the file, the line and the \
                   offending key, which should make the next review much quicker for you. "
        .repeat(3);
    other
        .request_rereview(&review, &summary, &agent_actor(), None)
        .unwrap();
    nudge(&mut shell);
    let text = banner(&mut shell, &tab, BannerKind::Rereview).unwrap();
    assert!(text.ends_with('…'), "{text}");
    assert!(text.chars().count() < 220, "{text}");
    // Both buttons stay inside the window: the texts shrink and truncate.
    let width = shell.cx.update(|window, _| window.viewport_size().width);
    for name in ["banner-button-0", "banner-button-1"] {
        let b = shell
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not painted"));
        assert!(b.right() <= width, "{name} at {b:?}, window {width:?}");
    }
}

#[test]
fn banner_texts_name_the_agents_and_count_threads() {
    use polygloss_core::review::{Rereview, UnreadThread};
    let unread = |names: &[Option<&str>]| -> Vec<UnreadThread> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| UnreadThread {
                thread_id: format!("t{i}"),
                last_seq: i as i64 + 1,
                actor_name: n.map(str::to_owned),
            })
            .collect()
    };
    assert_eq!(feed::replies_text(&[]), None);
    assert_eq!(
        feed::replies_text(&unread(&[Some("claude-code")])).as_deref(),
        Some("claude-code replied to 1 thread")
    );
    assert_eq!(
        feed::replies_text(&unread(&[Some("codex"), Some("claude-code")])).as_deref(),
        Some("Agents replied to 2 threads")
    );
    let r = |summary: &str, by: Option<&str>| Rereview {
        summary: summary.into(),
        at: 1,
        requested_by: by.map(str::to_owned),
    };
    assert_eq!(
        feed::rereview_text(&r("# Done\n\nAll *fixed*.", Some("claude-code"))),
        "claude-code requested a re-review: Done"
    );
    assert_eq!(
        feed::rereview_text(&r("  \n", None)),
        "The agent requested a re-review"
    );
    assert_eq!(
        feed::rereview_text(&r("- [x] `a` and <b>b</b>", Some("x"))),
        "x requested a re-review: a and <b>b</b>"
    );
}
