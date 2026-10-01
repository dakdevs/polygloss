//! GPUI tests of T3.17: notifications, Dock badge and mute (design §17).
//!
//! A re-review request posts a macOS notification only while the app is
//! unfocused, notifications are enabled (`notifications.enabled`), the
//! review is not muted and the app runs from a bundle; plain agent replies
//! never notify; the Dock badge counts the reviews awaiting you (re-review
//! requested or an open agent question), muted or not; clicking the
//! notification focuses the review's tab.
//!
//! The platform side is an injected [`Notifier`]: it says whether the app is
//! bundled, records badges, and posts through GPUI's test platform (which
//! records `shown_system_notifications` and simulates clicks), so no real
//! notification or Dock badge is touched.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{App, Entity, SystemNotification, SystemNotificationResponse};
use polygloss_app::notify::{self, Notifier};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::settings::SettingsStore;
use polygloss_app::tabs::TabItem;
use polygloss_core::git::Source;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, Subject, ThreadKind, Verdict,
};
use polygloss_core::store::events::{Actor, ActorKind};
use polygloss_diff::Side;

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

/// The injected platform: bundled or not, badges recorded, notifications
/// posted to GPUI's test platform.
#[derive(Default)]
struct Recorder {
    bundled: Cell<bool>,
    badges: RefCell<Vec<Option<String>>>,
}

impl Notifier for Recorder {
    fn bundled(&self) -> bool {
        self.bundled.get()
    }

    fn show(&self, notification: SystemNotification, cx: &mut App) {
        cx.show_system_notification(notification);
    }

    fn dismiss(&self, tag: &str, cx: &mut App) {
        cx.dismiss_system_notification(tag);
    }

    fn set_badge(&self, label: Option<&str>) {
        self.badges.borrow_mut().push(label.map(str::to_owned));
    }
}

/// Starts the app with a [`Recorder`] (bundled) in place of the system.
fn start_recording(cx: &mut gpui_kit::TestAppContext) -> (Shell<'_>, Rc<Recorder>) {
    let shell = start(cx);
    let recorder = Rc::new(Recorder::default());
    recorder.bundled.set(true);
    let r = recorder.clone();
    shell.cx.update(|_, cx| notify::set_notifier(r, cx));
    draw(shell.cx);
    (shell, recorder)
}

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

/// Another connection to the sandbox store, as `polygloss mcp` would open.
fn other_core() -> Core {
    Core::open_default().expect("open a second connection to the store")
}

fn commit_req(repo: &std::path::Path) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Commit {
            rev: "refs/tags/head".into(),
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn review_id(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, _| t.review_id.clone())
}

/// An agent thread of `kind` on line 5 of `src/config.rs` in `tab`'s diff.
fn agent_thread(
    core: &Core,
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
    kind: ThreadKind,
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
            subject: Subject::Line {
                path: "src/config.rs".into(),
                side: Side::New,
                start_line: 5,
                line: 5,
            },
            kind,
            body_md: body.into(),
            author: agent(),
        },
        &blobs,
    )
    .expect("create the agent thread")
}

/// Lets the feed see what another connection wrote, at once.
fn nudge(shell: &mut Shell) {
    shell.cx.update(|_, cx| polygloss_app::feed::nudge(cx));
    draw(shell.cx);
}

fn rereview(shell: &mut Shell, review: &str, summary: &str) {
    other_core()
        .request_rereview(review, summary, &agent_actor(), None)
        .expect("request a re-review");
    nudge(shell);
}

fn shown(shell: &mut Shell) -> Vec<SystemNotification> {
    shell.cx.shown_system_notifications()
}

fn set_notifications_enabled(shell: &mut Shell, enabled: bool) {
    shell.cx.update(|_, cx| {
        let mut settings = SettingsStore::global(cx).settings().clone();
        settings.notifications.enabled = enabled;
        SettingsStore::set(settings, cx);
    });
}

fn badge(shell: &mut Shell) -> u32 {
    shell.cx.update(|_, cx| notify::badge_count(cx))
}

fn active_review(shell: &mut Shell) -> Option<String> {
    let tab = shell.active_review()?;
    Some(review_id(shell, &tab))
}

#[gpui_kit::test]
fn notification_only_when_unfocused_unmuted_and_bundled(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut shell, recorder) = start_recording(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);

    // Focused: the banner is enough.
    shell.cx.update(|window, _| window.activate_window());
    rereview(&mut shell, &review, "Focused, so no notification.");
    assert!(shown(&mut shell).is_empty(), "focused app: no notification");

    // Unfocused, enabled, unmuted, bundled: one notification.
    shell.cx.deactivate_window();
    rereview(
        &mut shell,
        &review,
        "Switched to **BTreeMap** as asked.\n\nAlso added `is_empty`.",
    );
    let posted = shown(&mut shell);
    assert_eq!(posted.len(), 1, "{posted:?}");
    let n = &posted[0];
    assert_eq!(n.tag.as_ref(), notify::rereview_tag(&review));
    assert_eq!(n.title.as_ref(), "claude-code requested a re-review");
    assert!(
        n.body.contains("head") && n.body.contains("Switched to BTreeMap as asked."),
        "body names the review and the summary: {:?}",
        n.body
    );
    assert!(
        !n.body.contains("**"),
        "markdown marks dropped: {:?}",
        n.body
    );

    // Notifications disabled in the settings (global mute).
    set_notifications_enabled(&mut shell, false);
    rereview(&mut shell, &review, "Disabled.");
    assert_eq!(shown(&mut shell).len(), 1, "notifications.enabled = false");
    set_notifications_enabled(&mut shell, true);

    // The review muted.
    shell.core.set_muted(&review, true).unwrap();
    rereview(&mut shell, &review, "Muted.");
    assert_eq!(shown(&mut shell).len(), 1, "muted review");
    shell.core.set_muted(&review, false).unwrap();

    // Not bundled: `show_system_notification` would abort the process.
    recorder.bundled.set(false);
    rereview(&mut shell, &review, "Unbundled.");
    assert_eq!(shown(&mut shell).len(), 1, "not bundled");

    // Everything allows it again.
    recorder.bundled.set(true);
    rereview(&mut shell, &review, "Again.");
    assert_eq!(shown(&mut shell).len(), 2);
}

#[gpui_kit::test]
fn plain_agent_reply_never_notifies(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut shell, _recorder) = start_recording(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    shell.cx.deactivate_window();

    let core = other_core();
    let thread = agent_thread(&core, &mut shell, &tab, ThreadKind::Comment, "A note.");
    nudge(&mut shell);
    core.reply(&thread, "Another reply.", &agent()).unwrap();
    nudge(&mut shell);
    core.set_resolved(&thread, true, &agent_actor(), Some("Done."))
        .unwrap();
    nudge(&mut shell);
    // A question awaits you (badge) but does not notify either.
    agent_thread(&core, &mut shell, &tab, ThreadKind::Question, "Which one?");
    nudge(&mut shell);

    assert!(shown(&mut shell).is_empty(), "{:?}", shown(&mut shell));
    assert_eq!(badge(&mut shell), 1);
}

#[gpui_kit::test]
fn mute_suppresses_notification_not_badge(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut shell, recorder) = start_recording(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    shell.core.set_muted(&review, true).unwrap();
    shell.cx.deactivate_window();

    rereview(&mut shell, &review, "Please look again.");

    assert!(shown(&mut shell).is_empty());
    assert_eq!(badge(&mut shell), 1);
    assert_eq!(
        recorder
            .badges
            .borrow()
            .last()
            .cloned()
            .flatten()
            .as_deref(),
        Some("1")
    );
}

#[gpui_kit::test]
fn badge_counts_rereview_and_open_questions(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut shell, recorder) = start_recording(cx);
    let a = shell.open(compare_req(repo.path())).unwrap();
    let b = shell.open(commit_req(repo.path())).unwrap();
    let (review_a, review_b) = (review_id(&mut shell, &a), review_id(&mut shell, &b));
    assert_ne!(review_a, review_b);
    assert_eq!(badge(&mut shell), 0);
    assert!(recorder.badges.borrow().is_empty(), "nothing to clear yet");

    // An open agent question on A.
    let core = other_core();
    let question = agent_thread(&core, &mut shell, &a, ThreadKind::Question, "Which one?");
    nudge(&mut shell);
    assert_eq!(badge(&mut shell), 1);

    // A re-review request on B.
    rereview(&mut shell, &review_b, "Fixed.");
    assert_eq!(badge(&mut shell), 2);

    // The human answers A's question (a submitted reply).
    shell
        .core
        .reply(
            &question,
            "This one.",
            &Author {
                kind: AuthorKind::Human,
                name: "you".into(),
                session_id: None,
            },
        )
        .unwrap();
    shell
        .core
        .submit_review(&review_a, Verdict::Comment, "", None)
        .unwrap();
    nudge(&mut shell);
    assert_eq!(badge(&mut shell), 1);

    // Submitting B ends its re-review request.
    shell
        .core
        .submit_review(&review_b, Verdict::Approve, "", None)
        .unwrap();
    nudge(&mut shell);
    assert_eq!(badge(&mut shell), 0);

    assert_eq!(
        *recorder.badges.borrow(),
        [Some("1".into()), Some("2".into()), Some("1".into()), None]
    );
}

#[gpui_kit::test]
fn notification_click_focuses_review_tab(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut shell, _recorder) = start_recording(cx);
    let a = shell.open(compare_req(repo.path())).unwrap();
    let review_a = review_id(&mut shell, &a);
    shell.open(commit_req(repo.path())).unwrap();
    assert_ne!(
        active_review(&mut shell).as_deref(),
        Some(review_a.as_str())
    );

    shell.cx.deactivate_window();
    rereview(&mut shell, &review_a, "Please look again.");
    let tag = shown(&mut shell)[0].tag.clone();

    // Clicking it brings the window forward with A's tab.
    shell
        .cx
        .simulate_system_notification_response(SystemNotificationResponse {
            tag: tag.clone(),
            action_id: None,
        });
    draw(shell.cx);
    assert!(shell.cx.update(|_, cx| cx.active_window().is_some()));
    assert_eq!(
        active_review(&mut shell).as_deref(),
        Some(review_a.as_str())
    );

    // With A's tab closed, the click opens it again.
    let ix = shell
        .main
        .read_with(shell.cx, |m, cx| m.tabs().find_review(&review_a, cx))
        .unwrap();
    shell.main.update_in(shell.cx, |m, window, cx| {
        m.activate_tab(ix, window, cx);
    });
    shell.cx.dispatch_action(polygloss_app::tabs::CloseTab);
    draw(shell.cx);
    let open = shell
        .main
        .read_with(shell.cx, |m, cx| m.tabs().find_review(&review_a, cx));
    assert_eq!(open, None, "A's tab is closed");
    shell.main.update_in(shell.cx, |m, window, cx| {
        m.activate_tab(0, window, cx);
    });
    assert!(matches!(shell.tab(0), TabItem::Home(_)));

    shell
        .cx
        .simulate_system_notification_response(SystemNotificationResponse {
            tag,
            action_id: None,
        });
    draw(shell.cx);
    assert_eq!(
        active_review(&mut shell).as_deref(),
        Some(review_a.as_str())
    );
}

#[test]
fn rereview_tags_name_their_review() {
    let tag = notify::rereview_tag("r-123");
    assert_eq!(notify::review_of_tag(&tag), Some("r-123"));
    assert_eq!(notify::review_of_tag("rereview:"), None);
    assert_eq!(notify::review_of_tag("other:r-123"), None);
}

/// MCP `request_rereview` launches a closed app in the background with the
/// review's URL; the store feed of a fresh app starts after the request, so
/// the URL itself delivers the notification (T4.6).
#[gpui_kit::test]
fn review_url_delivers_a_pending_rereview_notification(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let (mut shell, _recorder) = start_recording(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review = review_id(&mut shell, &tab);
    shell.cx.deactivate_window();
    let url = polygloss_core::urls::format_url(&polygloss_core::urls::PolyglossUrl::Review(
        review.clone(),
    ));
    let open_url = |shell: &mut Shell, url: &str| {
        let task = shell
            .cx
            .update(|_, cx| polygloss_app::urls::open_url(url, cx));
        draw(shell.cx);
        futures::FutureExt::now_or_never(task).expect("the URL open finished")
    };

    // No request pending: opening the review posts nothing.
    open_url(&mut shell, &url).unwrap();
    assert!(shown(&mut shell).is_empty());

    // A request the feed has not read (no nudge): the URL delivers it.
    other_core()
        .request_rereview(&review, "Fixed as asked.", &agent_actor(), None)
        .expect("request a re-review");
    open_url(&mut shell, &url).unwrap();
    let posted = shown(&mut shell);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(posted[0].tag.as_ref(), notify::rereview_tag(&review));

    // Muted: nothing more.
    shell.core.set_muted(&review, true).unwrap();
    open_url(&mut shell, &url).unwrap();
    assert_eq!(shown(&mut shell).len(), 1);
}

#[test]
fn system_notifier_never_posts_in_test_mode() {
    // A bundled app in test mode (POLYGLOSS_TEST=1: bun E2E suites run the
    // release bundle's executable, plan T5.7) must never reach the user's
    // Notification Center, whose authorization prompt and settings are not
    // sandboxed by HOME.
    use polygloss_app::notify::SystemNotifier;
    assert!(SystemNotifier::for_process(true, false).bundled());
    assert!(!SystemNotifier::for_process(true, true).bundled());
    assert!(!SystemNotifier::for_process(false, false).bundled());
    assert!(!SystemNotifier::for_process(false, true).bundled());
}

#[test]
fn system_notifier_reads_test_mode_from_polygloss_test() {
    // `SystemNotifier::new()` takes test mode from the process environment:
    // exactly `POLYGLOSS_TEST=1` (the variable the bun E2E suites set).
    use polygloss_app::notify::SystemNotifier;
    use std::ffi::OsString;
    let with = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| OsString::from(v))
        }
    };
    assert!(!SystemNotifier::from_lookup(true, with(&[("POLYGLOSS_TEST", "1")])).bundled());
    assert!(SystemNotifier::from_lookup(true, with(&[])).bundled());
    assert!(SystemNotifier::from_lookup(true, with(&[("POLYGLOSS_TEST", "0")])).bundled());
    assert!(SystemNotifier::from_lookup(true, with(&[("POLYGLOSS_TEST", "")])).bundled());
    // Another variable does not count.
    assert!(SystemNotifier::from_lookup(true, with(&[("POLYGLOSS_E2E", "1")])).bundled());
    assert!(!SystemNotifier::from_lookup(false, with(&[])).bundled());
}
