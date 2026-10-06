//! Submit review (design §8.3, §11.4, ADR-0011): the toolbar's "Submit
//! review" button with the draft count, `⌘⇧⏎` (`tab::SubmitReview`) and the
//! Review menu open the dialog ([`dialog`]). Submitting publishes every
//! draft of the review with the verdict in one transaction (T1.13), pinning
//! a live tab's displayed state first; zero drafts are fine (to approve).
//! The dialog's waiter line says whether the assigned agent session is
//! waiting (`Core::live_waiter_for_review`).
//!
//! Resolve and unresolve act at once and live with the thread cards
//! (`crate::composer::set_resolved`).

pub mod dialog;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    KeyBinding, MenuItem, ParentElement as _, Styled as _, Subscription, WeakEntity, Window, div,
    px,
};

pub use dialog::{ConfirmSubmit, DialogInit, SubmitDialog, SubmitEvent, WaiterState};

use crate::app_state::AppState;
use crate::keymap::actions::tab as tab_actions;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::review_tab::toolbar::{self, Narrow};
use crate::space::{TextStyleExt as _, gap, height, layout, pad, radius, text};
use crate::threads;
use crate::window::MenuKind;

/// Registers `⌘⇧⏎` / "Submit Review…", and `⌘⏎` in the dialog.
pub fn init(cx: &mut App) {
    // The dialog's own key (not a keymap action, like the find bar's
    // toggles): ⌘⏎ submits wherever the keyboard is in it.
    // ⌘1 / ⌘2 / ⌘3 pick the verdict (T5.6; ⇥ and Space reach the radios
    // too).
    let ctx = Some(dialog::CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-enter", ConfirmSubmit, ctx),
        KeyBinding::new("cmd-1", dialog::VerdictComment, ctx),
        KeyBinding::new("cmd-2", dialog::VerdictApprove, ctx),
        KeyBinding::new("cmd-3", dialog::VerdictRequestChanges, ctx),
    ]);
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::SubmitReview, window, cx| {
            open_dialog(tab, window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::Review,
        vec![
            MenuItem::separator(),
            MenuItem::action("Submit Review…", tab_actions::SubmitReview),
        ],
        cx,
    );
}

/// The tab's open dialog.
#[derive(Default)]
struct SubmitState {
    dialog: Option<WeakEntity<SubmitDialog>>,
    opening: bool,
    _subscription: Option<Subscription>,
}

/// Nothing to set up until the dialog opens.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, _cx: &mut Context<ReviewTab>) {
    tab.insert_extension(SubmitState::default());
}

/// The open Submit review dialog of `tab`.
pub fn dialog(tab: &ReviewTab) -> Option<Entity<SubmitDialog>> {
    tab.extension::<SubmitState>()?.dialog.as_ref()?.upgrade()
}

/// The review's draft comments: unpublished comments of its own threads
/// (a reply drafted on another review's thread would be that review's,
/// OQ-P16, and is never offered).
pub fn drafts_count(tab: &ReviewTab, cx: &App) -> u32 {
    let Some(model) = threads::threads(tab) else {
        return 0;
    };
    model
        .read(cx)
        .threads()
        .filter(|t| t.review_id.as_deref() == Some(tab.review_id.as_str()))
        .flat_map(|t| t.comments.iter())
        .filter(|c| c.draft && !c.deleted)
        .count() as u32
}

/// Opens the Submit review dialog (focusing the open one): the autosaved
/// summary and verdict and the waiter state are read on the background
/// executor first.
pub fn open_dialog(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    // (A dialog closed with Esc lives on for a frame.)
    if let Some(open) = dialog(tab)
        && window.has_active_dialog(cx)
    {
        let handle = open.read(cx).summary_focus(cx);
        window.focus(&handle, cx);
        return;
    }
    let Some(state) = tab.extension_mut::<SubmitState>() else {
        return;
    };
    if state.opening {
        return;
    }
    state.opening = true;
    let core = AppState::global(cx).core.clone();
    let review_id = tab.review_id.clone();
    let drafts = drafts_count(tab, cx);
    let live = tab
        .opened
        .live
        .clone()
        .map(|state| (tab.opened.base.clone(), state));
    let work = cx.background_spawn({
        let review_id = review_id.clone();
        async move {
            let saved = core.submit_draft(&review_id)?;
            let waiter = core.live_waiter_for_review(&review_id)?;
            let assigned = core.assigned_session(&review_id)?;
            let waiter = match (waiter, assigned) {
                (Some(_), Some(s)) => WaiterState::Listening {
                    agent: s.client_name,
                },
                (Some(_), None) => WaiterState::Listening {
                    agent: "The agent".to_owned(),
                },
                (None, Some(s)) => WaiterState::NotListening {
                    agent: s.client_name,
                },
                (None, None) => WaiterState::Unassigned,
            };
            anyhow::Ok((saved, waiter))
        }
    });
    cx.spawn_in(window, async move |tab, cx| {
        let result = work.await;
        tab.update_in(cx, |tab, window, cx| {
            if let Some(state) = tab.extension_mut::<SubmitState>() {
                state.opening = false;
            }
            match result {
                Ok((saved, waiter)) => show_dialog(
                    tab,
                    DialogInit {
                        review_id,
                        drafts,
                        saved,
                        waiter,
                        live,
                    },
                    window,
                    cx,
                ),
                Err(e) => {
                    tracing::warn!("opening the submit dialog: {e:#}");
                    if let Some((_, main)) = crate::window::main_window(cx) {
                        main.update(cx, |main, cx| {
                            main.toast_error(
                                format!("Could not open Submit review: {e}").into(),
                                window,
                                cx,
                            )
                        });
                    }
                }
            }
        })
        .ok();
    })
    .detach();
}

fn show_dialog(
    tab: &mut ReviewTab,
    init: DialogInit,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let view = cx.new(|cx| SubmitDialog::new(init, window, cx));
    let subscription = cx.subscribe_in(
        &view,
        window,
        |tab: &mut ReviewTab, _, event: &SubmitEvent, window, cx| match event {
            SubmitEvent::Submitted(sub) => submitted(tab, sub, window, cx),
        },
    );
    if let Some(state) = tab.extension_mut::<SubmitState>() {
        state.dialog = Some(view.downgrade());
        state._subscription = Some(subscription);
    }
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Submit review")
            .w(px(layout::DIALOG_W))
            .child(content.clone())
    });
    let handle = view.read(cx).summary_focus(cx);
    window.focus(&handle, cx);
}

/// The review was submitted: the dialog closes, a pinned live state becomes
/// the tab's iteration, the threads (no longer drafts) reload.
fn submitted(
    tab: &mut ReviewTab,
    sub: &polygloss_core::review::Submission,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    if let Some(state) = tab.extension_mut::<SubmitState>() {
        state.dialog = None;
        state._subscription = None;
    }
    window.close_dialog(cx);
    if sub.iteration.diff_id == tab.opened.diff_id {
        tab.opened.iteration = Some(sub.iteration.clone());
    }
    threads::reload(tab, cx);
    let verdict = dialog::verdict_text(sub.verdict).0;
    let message = match sub.comment_count {
        0 => format!("Review submitted: {verdict}"),
        1 => format!("Review submitted: {verdict}, 1 comment published"),
        n => format!("Review submitted: {verdict}, {n} comments published"),
    };
    if let Some((_, main)) = crate::window::main_window(cx) {
        main.update(cx, |main, cx| {
            main.toast_success(message.into(), window, cx)
        });
    }
    window.focus(&tab.viewport_focus().clone(), cx);
    cx.notify();
}

/// The toolbar's "Submit review" button with the draft count (design
/// §11.4), "Submit" once the toolbar is narrow; in the accent color.
pub fn button(tab: &ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) -> AnyElement {
    let drafts = drafts_count(tab, cx);
    let label = if toolbar::narrow(tab) >= Narrow::ShortSubmit {
        "Submit"
    } else {
        "Submit review"
    };
    let theme = cx.theme();
    // A badge (`MINI`, at least round), `INLINE` after the label.
    let count = (drafts > 0).then(|| {
        div()
            .debug_selector(|| "submit-review-count".into())
            .ml(px(gap::INLINE))
            .px(px(pad::BADGE_X))
            .min_w(px(height::MINI))
            .h(px(height::MINI))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::capsule(height::MINI)))
            .bg(theme.primary_foreground.opacity(0.22))
            .text_style(text::CAPTION)
            .child(drafts.to_string())
    });
    Button::new("submit-review")
        .debug_selector(|| "submit-review".into())
        .small()
        .primary()
        .child(toolbar::text("submit-review-label", label))
        .when_some(count, |b, c| b.child(c))
        .tooltip(match drafts {
            0 => "Submit a verdict (⌘⇧⏎)".to_owned(),
            1 => "Publish 1 draft with a verdict (⌘⇧⏎)".to_owned(),
            n => format!("Publish {n} drafts with a verdict (⌘⇧⏎)"),
        })
        .on_click(cx.listener(|tab, _, window, cx| open_dialog(tab, window, cx)))
        .into_any_element()
}
