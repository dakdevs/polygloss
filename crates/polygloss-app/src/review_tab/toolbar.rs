//! The review tab's toolbar (design §11.4), the main column's top row
//! ([`crate::chrome::toolbar_row`]: it moves the window). The shell draws the
//! review's title and kind on the left and the threads panel toggle
//! ([`crate::review_tab::panes::ToggleThreadsPanel`]) on the right;
//! features contribute their controls in §11.4's order through their
//! `toolbar_items` functions:
//!
//! iteration picker (T3.12) · base picker and Snapshot (T3.11) · view toggles
//! (T3.2) · "N / M viewed" (T3.7) · Hide agent notes (T3.9) · drafts +
//! Submit review (T3.10).

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div,
};

use crate::review_tab::ReviewTab;

/// The review tab's toolbar row ([`crate::chrome::toolbar_row`], debug
/// selector `review-toolbar`).
pub(crate) fn render(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let mut left: Vec<AnyElement> = Vec::new();
    left.extend(crate::iterations::toolbar_items(tab, window, cx));
    left.extend(crate::live::toolbar_items(tab, window, cx));
    let mut right: Vec<AnyElement> = Vec::new();
    right.extend(crate::palette::toolbar_items(tab, window, cx));
    right.extend(crate::viewed::toolbar_items(tab, window, cx));
    right.extend(crate::threads::toolbar_items(tab, window, cx));
    right.extend(crate::submit::toolbar_items(tab, window, cx));

    let theme = cx.theme();
    let visible = tab.panes.threads_visible;
    let kind = div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded(theme.radius)
        .bg(theme.secondary)
        .text_xs()
        .font_medium()
        .text_color(theme.secondary_foreground)
        .child(tab.opened.kind.as_str().to_uppercase());
    let title = div()
        .min_w_0()
        .truncate()
        .text_sm()
        .font_semibold()
        .text_color(theme.foreground)
        .child(tab.title());
    let toggle_threads = Button::new("toggle-threads-panel")
        .icon(if visible {
            IconName::PanelRightClose
        } else {
            IconName::PanelRightOpen
        })
        .small()
        .ghost()
        .tooltip(if visible {
            "Hide threads panel"
        } else {
            "Show threads panel"
        })
        .debug_selector(|| "toggle-threads-panel".into())
        // Straight to the tab, not an action dispatched from the focused
        // element: the click works wherever focus is.
        .on_click(cx.listener(|tab, _, _, cx| tab.toggle_threads_panel(cx)));
    let left = [kind.into_any_element(), title.into_any_element()]
        .into_iter()
        .chain(left)
        .collect();
    right.push(toggle_threads.into_any_element());
    crate::chrome::toolbar_row("review-toolbar", left, right, window, cx)
}
