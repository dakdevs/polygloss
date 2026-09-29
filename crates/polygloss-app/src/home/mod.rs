//! Home: recent reviews across repos, "Awaiting you" first (design §11.2).
//!
//! Owned by T3.4. Created as a stub by T3.1: the main window's first tab is
//! a [`HomeView`] (made with [`HomeView::new`]) and [`init`] is called from
//! `features::init`. Until T3.4 it shows an empty state.

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, Window, px,
};

/// Registers Home's actions.
pub fn init(_cx: &mut App) {}

/// The Home tab's view.
pub struct HomeView {
    focus: FocusHandle,
}

impl HomeView {
    pub fn new(_window: &mut Window, cx: &mut App) -> Entity<HomeView> {
        cx.new(|cx| HomeView {
            focus: cx.focus_handle(),
        })
    }
}

impl Focusable for HomeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HomeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .key_context("Home")
            .track_focus(&self.focus)
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(theme.background)
            .child(
                gpui_kit::div()
                    .text_size(px(15.))
                    .text_color(theme.foreground)
                    .child("No reviews yet"),
            )
            .child(
                gpui_kit::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Run `polygloss` in a repository, or press ⌘O to open one."),
            )
    }
}
