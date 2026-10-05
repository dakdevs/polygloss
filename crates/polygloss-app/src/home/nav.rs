//! The sidebar's Reviews segment (design §11.2): a Home row, then one row
//! per open review. A click activates the row's tab; × closes its review
//! as ⌘W does. T6.3's stub; T6.6 adds the Open heading's Open…, Awaiting
//! you and Recent, the row menus and the styling.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::tabs::TabItem;

/// The Reviews segment's list.
pub fn render_nav(_window: &mut Window, _cx: &mut App) -> AnyElement {
    // Rendered as its own element: a review tab's sidebar calls this while
    // the tab renders, when the tab's own entity cannot be read; the rows
    // read the tabs' titles once that render has returned.
    Nav.into_any_element()
}

#[derive(IntoElement)]
struct Nav;

/// One row of the list.
struct NavRow {
    /// Its tab's index.
    ix: usize,
    /// `nav-home`, or the review's id.
    review_id: Option<String>,
    title: SharedString,
    active: bool,
}

impl RenderOnce for Nav {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let list = v_flex()
            .debug_selector(|| "nav".into())
            .size_full()
            .px_2()
            .pt_1()
            .gap_0p5();
        let Some((_, main)) = crate::window::main_window(cx) else {
            return list;
        };
        let rows: Vec<NavRow> = {
            let tabs = main.read(cx).tabs();
            tabs.items()
                .iter()
                .enumerate()
                .map(|(ix, item)| NavRow {
                    ix,
                    review_id: match item {
                        TabItem::Home(_) => None,
                        TabItem::Review(tab) => Some(tab.read(cx).review_id.clone()),
                    },
                    title: item.title(cx),
                    active: ix == tabs.active(),
                })
                .collect()
        };
        let main = main.downgrade();
        list.children(rows.into_iter().map(|row| {
            let theme = cx.theme();
            let selector = match &row.review_id {
                Some(id) => format!("open-review-{id}"),
                None => "nav-home".to_owned(),
            };
            let ix = row.ix;
            let close = row.review_id.as_ref().map(|id| {
                let close_selector = format!("close-review-{id}");
                let main = main.clone();
                // The row activates on click; × must not.
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new(("close-review", ix))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Close Review (⌘W)")
                            .debug_selector(move || close_selector)
                            .on_click(move |_, window, cx| {
                                main.update(cx, |m, cx| m.close_tab_at(ix, window, cx)).ok();
                            }),
                    )
            });
            let activate = main.clone();
            h_flex()
                .id(("nav-row", ix))
                .debug_selector(move || selector)
                .h(px(28.))
                .px_2()
                .gap_2()
                .rounded(theme.radius)
                .text_sm()
                .text_color(theme.sidebar_foreground)
                .when(row.active, |el| el.bg(theme.list_active))
                .when(!row.active, |el| el.hover(|s| s.bg(theme.list_hover)))
                .when(row.review_id.is_none(), |el| {
                    el.child(
                        gpui_kit::component::Icon::new(IconName::Inbox)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                })
                .child(div().flex_1().min_w_0().truncate().child(row.title))
                .children(close)
                .on_click(move |_, window, cx| {
                    activate
                        .update(cx, |m, cx| m.activate_tab(ix, window, cx))
                        .ok();
                })
        }))
    }
}
