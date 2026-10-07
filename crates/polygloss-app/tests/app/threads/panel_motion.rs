//! The threads panel's motion (T7.10, ADR-0030 M2): the sidebar's mirrored
//! from the window's trailing edge, the content laid out at W riding the
//! frame's leading edge (its divider), the viewport ending at the same
//! `f`, the toolbar still (the panel sits under it). Its frame is today's
//! pane, 340 pt with its 1 pt divider (W 339). The automatic opens snap.
//! Values are hand-written as in `shell::sidebar_motion`, whose helpers
//! this module uses.

use gpui_kit::{Bounds, Entity, Pixels, TestAppContext};
use polygloss_app::keymap::actions::tab as tab_actions;
use polygloss_app::motion::tokens::FIRST_STEP;
use polygloss_app::review_tab::{BannerKind, ReviewTab};
use polygloss_core::review::Subject;
use polygloss_core::urls::UrlFocus;

use crate::shell::sidebar_motion::{CLOSE_MS, OPEN_MS, click_now, ms, q, review, slide};
use crate::shell::{Shell, bounds, draw, painted};
use crate::support::Sandbox;
use crate::support::motion::{advance, requested_frames, step_to};

/// W̄: the pane, its divider included.
const WBAR: f64 = 340.0;

/// The panel's frame: from the viewport's end to the window's.
fn frame_width(shell: &mut Shell) -> f32 {
    let window = shell.cx.update(|window, _| window.viewport_size().width);
    (window - bounds(shell.cx, "viewport-pane").right()).as_f32()
}

fn shown(shell: &mut Shell, tab: &Entity<ReviewTab>) -> bool {
    tab.read_with(shell.cx, |t, _| t.threads_panel_visible())
}

/// The toolbar's row and its first and last items.
fn toolbar(shell: &mut Shell) -> [Bounds<Pixels>; 3] {
    ["review-toolbar", "repo-block", "submit-review"].map(|name| bounds(shell.cx, name))
}

/// The divider, the content and the frame derive from one `f`; the
/// toolbar and the diff's scroll are where they were.
fn check(
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
    still: &([Bounds<Pixels>; 3], f64),
    at: &str,
) -> f32 {
    let f = frame_width(shell);
    let window = shell.cx.update(|window, _| window.viewport_size().width);
    if f > 0.0 {
        assert_eq!(
            bounds(shell.cx, "threads-divider").left(),
            window - gpui_kit::px(f),
            "{at}: the divider is the frame's first point"
        );
        assert_eq!(
            bounds(shell.cx, "threads-panel").left(),
            window - gpui_kit::px(f) + gpui_kit::px(1.),
            "{at}: the content rides it"
        );
        assert_eq!(
            bounds(shell.cx, "threads-pane").size.width,
            gpui_kit::px(f),
            "{at}: the frame"
        );
    } else {
        assert!(painted(shell.cx, "threads-panel").is_none(), "{at}");
    }
    assert_eq!(toolbar(shell), still.0, "{at}: the toolbar does not move");
    let top = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().scroll_top()
    });
    assert_eq!(top, still.1, "{at}: the diff's scroll holds");
    f
}

#[gpui_kit::test]
fn threads_panel_mirrors_the_sidebar(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    let viewport = bounds(shell.cx, "viewport-pane").center();
    shell.cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: viewport,
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(-60.))),
        modifiers: gpui_kit::Modifiers::none(),
        touch_phase: gpui_kit::TouchPhase::Moved,
    });
    draw(shell.cx);
    let top = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().scroll_top()
    });
    assert!(top > 0.0, "the diff scrolled");
    let still = (toolbar(&mut shell), top);

    // An open, by the toolbar's button.
    click_now(&mut shell, "toggle-threads-panel");
    assert!(
        shown(&mut shell, &tab),
        "the model shows it from the commit"
    );
    assert_eq!(check(&mut shell, &tab, &still, "open, commit"), 0.0);
    step_to(shell.cx, FIRST_STEP);
    assert_eq!(
        check(&mut shell, &tab, &still, "open, the first step"),
        q(WBAR * slide(16.667 / OPEN_MS))
    );
    advance(shell.cx, ms(OPEN_MS / 2.0) - FIRST_STEP);
    assert_eq!(
        check(&mut shell, &tab, &still, "open, ½"),
        q(WBAR * slide(0.5))
    );
    advance(shell.cx, ms(OPEN_MS / 2.0));
    assert_eq!(check(&mut shell, &tab, &still, "open, settled"), 340.0);
    assert_eq!(requested_frames(shell.cx), 0);

    // A close: its exit duration, back to the trailing edge.
    click_now(&mut shell, "toggle-threads-panel");
    assert!(!shown(&mut shell, &tab));
    assert_eq!(check(&mut shell, &tab, &still, "close, commit"), 340.0);
    step_to(shell.cx, ms(CLOSE_MS / 2.0));
    assert_eq!(
        check(&mut shell, &tab, &still, "close, ½"),
        q(WBAR * (1.0 - slide(0.5)))
    );
    advance(shell.cx, ms(CLOSE_MS / 2.0));
    assert_eq!(check(&mut shell, &tab, &still, "close, settled"), 0.0);
    assert_eq!(requested_frames(shell.cx), 0);
}

#[gpui_kit::test]
fn automatic_threads_opens_snap(cx: &mut TestAppContext) {
    use polygloss_app::view_state;
    use polygloss_core::review::ViewState;
    let _sb = Sandbox::isolate();
    let (mut shell, tab) = review(cx);
    let snapped = |shell: &mut Shell, how: &str| {
        use polygloss_app::panel_split::PanelOwner as _;
        assert!(shown(shell, &tab), "{how}: shown");
        assert_eq!(frame_width(shell), 340.0, "{how}: at its end at once");
        let panel = tab.read_with(shell.cx, |t, _| t.threads_panel().clone());
        assert!(
            !panel.read_with(shell.cx, |p, _| p.panel().moving()),
            "{how}: no motion runs"
        );
        // Other surfaces may ask for frames of their own here (the
        // composer's caret, the banner's exit), so a later frame shows the
        // panel has nowhere to go.
        advance(shell.cx, FIRST_STEP);
        assert_eq!(frame_width(shell), 340.0, "{how}: and stays");
    };
    let hide = |shell: &mut Shell| {
        shell.cx.simulate_keystrokes("escape");
        tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(false, cx));
        draw(shell.cx);
        assert_eq!(frame_width(shell), 0.0);
    };

    // The composer: Comment on review opens the panel with its composer.
    shell.cx.dispatch_action(tab_actions::CommentOnReview);
    draw(shell.cx);
    snapped(&mut shell, "the composer");
    hide(&mut shell);

    // Agent replies: "Show" on the banner opens a review thread there.
    let other = crate::feed::other_core();
    let thread = crate::feed::agent_thread(&other, &mut shell, &tab, Subject::Review, "Done.");
    crate::feed::nudge(&mut shell);
    crate::feed::click_banner(&mut shell, &tab, BannerKind::AgentReplies);
    snapped(&mut shell, "agent replies");
    hide(&mut shell);

    // A `polygloss://` URL's thread, which only the panel shows.
    shell.cx.update(|window, cx| {
        polygloss_app::urls::apply_focus(&tab, UrlFocus::Thread(thread.clone()), window, cx)
            .expect("focus the thread")
    });
    draw(shell.cx);
    snapped(&mut shell, "a URL");
    hide(&mut shell);

    // MCP `focus` on it.
    let review_id = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    crate::ipc::run(
        &mut shell,
        polygloss_core::ipc::Op::Focus {
            review_id: Some(review_id),
            diff_id: None,
            path: None,
            side: None,
            line: None,
            thread_id: Some(thread),
        },
    )
    .expect("focus");
    draw(shell.cx);
    snapped(&mut shell, "focus");
    hide(&mut shell);

    // Restore: the view state that saved it shown.
    tab.update(shell.cx, |t, cx| {
        view_state::restore(
            t,
            &ViewState {
                threads_panel: Some(true),
                ..ViewState::default()
            },
            cx,
        )
    });
    draw(shell.cx);
    snapped(&mut shell, "restore");
}
