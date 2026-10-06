//! The file finder (⌘P, design §11.8): every changed path of the review tab,
//! ranked by `nucleo-matcher` as you type (a gpui-kit `List` whose search
//! runs the matcher; the built-in `Command` filter is substring-only).
//! Enter jumps to the file: the tree selects it, the viewport scrolls to
//! its header and gets the keyboard.

use std::sync::Arc;

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, StyledExt as _, WindowExt as _, h_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Task, WeakEntity, Window, div, px,
};
use polygloss_diff::FileChange;
use polygloss_viewport::ScrollTarget;

use super::filters::Fuzzy;
use super::row::status_badge;
use crate::review_tab::ReviewTab;
use crate::space::{TextStyleExt as _, edge, gap, height, layout, pad, radius, size, text};

/// The files of `order` (display order) matching `query`, best first, ties
/// in display order; all of them for a blank query.
pub fn rank(files: &[FileChange], order: &[u32], query: &str) -> Vec<u32> {
    let Some(mut fuzzy) = Fuzzy::new(query) else {
        return order.to_vec();
    };
    let mut scored: Vec<(u32, u32)> = order
        .iter()
        .filter_map(|&i| Some((fuzzy.score(files.get(i as usize)?.display_path())?, i)))
        .collect();
    // Stable: equal scores keep display order.
    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// The finder's list: the ranked files of one tab.
pub struct FinderDelegate {
    files: Arc<Vec<FileChange>>,
    /// The files in display order.
    order: Vec<u32>,
    matches: Vec<u32>,
    selected: Option<usize>,
    tab: WeakEntity<ReviewTab>,
}

impl FinderDelegate {
    /// The files listed, in order.
    pub fn matches(&self) -> &[u32] {
        &self.matches
    }

    /// The row Enter would open.
    pub fn selected_file(&self) -> Option<u32> {
        self.matches.get(self.selected?).copied()
    }
}

/// The finder's list state.
pub type Finder = Entity<ListState<FinderDelegate>>;

/// The finder while it is open (tests read it).
#[derive(Default)]
struct OpenFinder(Option<WeakEntity<ListState<FinderDelegate>>>);

impl Global for OpenFinder {}

/// The open finder, if any.
pub fn current(cx: &App) -> Option<Finder> {
    cx.try_global::<OpenFinder>()?.0.as_ref()?.upgrade()
}

/// Opens the finder over `tab`'s files, its search field focused.
pub fn open(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) -> Finder {
    let files = tab.opened.files.clone();
    let order = tab.viewport.read(cx).display_order().to_vec();
    let delegate = FinderDelegate {
        matches: order.clone(),
        order,
        files,
        selected: None,
        tab: cx.entity().downgrade(),
    };
    let state = cx.new(|cx| {
        let mut state = ListState::new(delegate, window, cx).searchable(true);
        if !state.delegate().matches.is_empty() {
            state.set_selected_index(Some(IndexPath::default()), window, cx);
        }
        state
    });
    cx.set_global(OpenFinder(Some(state.downgrade())));
    let list = state.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(layout::PICKER_W))
            .margin_top(px(layout::PICKER_TOP))
            .close_button(false)
            .p_0()
            .child(
                // The picker's frame (ADR-0031): the dialog's whole content.
                div().debug_selector(|| "file-finder".into()).child(
                    List::new(&list)
                        .search_placeholder("Go to file…")
                        .max_h(px(layout::OVERLAY_MAX_H))
                        .p(px(edge::OVERLAY)),
                ),
            )
    });
    state.update(cx, |s, cx| s.focus(window, cx));
    state
}

/// Closes the finder and jumps to file `idx` of its tab.
fn jump(tab: &WeakEntity<ReviewTab>, idx: u32, window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
    cx.set_global(OpenFinder(None));
    let Some(tab) = tab.upgrade() else {
        return;
    };
    tab.update(cx, |tab, cx| {
        match super::file_tree(tab) {
            Some(tree) => tree.clone().update(cx, |t, cx| t.select_file(idx, cx)),
            None => tab
                .viewport
                .update(cx, |v, cx| v.scroll_to(ScrollTarget::File(idx), cx)),
        }
        window.focus(tab.viewport_focus(), cx);
    });
}

impl ListDelegate for FinderDelegate {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.matches = rank(&self.files, &self.order, query);
        // `ListState::start_search` picks the selection from its row cache,
        // which still describes the previous query (it is rebuilt at
        // render): after a query that matched nothing it clears the
        // selection even though this one matches. Select the first match
        // once it is done.
        cx.defer_in(window, |state, window, cx| {
            let first = (!state.delegate().matches.is_empty()).then(IndexPath::default);
            if state.selected_index() != first {
                state.set_selected_index(first, window, cx);
            }
        });
        cx.notify();
        Task::ready(())
    }

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.matches.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let idx = *self.matches.get(ix.row)?;
        let file = self.files.get(idx as usize)?;
        let path = file.display_path();
        let (dir, name) = match path.rsplit_once('/') {
            Some((dir, name)) => (dir, name),
            None => ("", path),
        };
        let (letter, color) = status_badge(file.status, cx);
        let theme = cx.theme();
        let selected = self.selected == Some(ix.row);
        Some(
            ListItem::new(("finder-file", idx as usize))
                .debug_selector(move || format!("finder-row-{}", ix.row))
                .selected(selected)
                .h(px(height::MD))
                .py_0()
                .px(px(pad::TEXT))
                .rounded(px(radius::for_height(height::MD)))
                .text_style(text::UI)
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(gap::ICON_LABEL))
                        .child(
                            div()
                                .w(px(size::STATUS_COL))
                                .flex_none()
                                .text_style(text::SMALL)
                                .font_semibold()
                                .text_color(color)
                                .child(letter),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(gap::CONTROLS))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(theme.foreground)
                                        .child(SharedString::from(name.to_owned())),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_style(text::SMALL)
                                        .text_color(theme.muted_foreground)
                                        .child(SharedString::from(dir.to_owned())),
                                ),
                        ),
                ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div()
            .py(px(gap::SECTION))
            .w_full()
            .flex()
            .justify_center()
            .text_style(text::UI)
            .text_color(cx.theme().muted_foreground)
            .child("No matching files")
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix.map(|ix| ix.row);
        cx.notify();
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        if let Some(idx) = self.selected_file() {
            let tab = self.tab.clone();
            jump(&tab, idx, window, cx);
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        window.close_dialog(cx);
        cx.set_global(OpenFinder(None));
    }
}
