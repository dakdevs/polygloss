//! The sidebar's Reviews segment (design §11.2, OQ-36): a Home row with the
//! awaiting count; **Open**, with Open… (⌘O) in its heading and one row per
//! open review (kind icon, `repo · summary`, a live dot, × on hover, the
//! active one highlighted); and while a review is active, Home's **Awaiting
//! you** and **Recent** as compact rows with Home's ⋯ and context menus
//! ([`HomeView::row_menu`]). A click opens or focuses the row's review; ×
//! closes it as ⌘W does. The list is for the mouse: the keyboard has the Home
//! page (⌘0) and ⌘1–⌘9.
//!
//! The lists are read from the window's tabs and [`HomeView`] (never a
//! second `HomeView`), and every row acts on its review by id when it runs,
//! so a tab closed or a list reloaded since the frame never makes it act on
//! another review.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, Div, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, RenderOnce, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, px,
};
use polygloss_core::git::ReviewKind;

use crate::home::HomeView;
use crate::home::row::HomeRow;
use crate::window::MainWindow;

/// A row's height.
const ROW_HEIGHT: f32 = 28.0;
/// The hover group of a row (its ×, its ⋯).
const ROW_GROUP: &str = "nav-row";

/// The Reviews segment's list.
pub fn render_nav(_window: &mut Window, _cx: &mut App) -> AnyElement {
    // Rendered as its own element: a review tab's sidebar calls this while
    // the tab renders, when the tab's own entity cannot be read; the rows
    // read the tabs once that render has returned.
    Nav.into_any_element()
}

#[derive(IntoElement)]
struct Nav;

/// A review in the list.
struct Item {
    review_id: String,
    kind: ReviewKind,
    title: SharedString,
}

/// What the list shows, read from the tabs and Home.
struct Lists {
    home_active: bool,
    /// The open reviews in tab order, and whether each is the active one.
    open: Vec<(Item, bool)>,
    awaiting_count: usize,
    /// Home's sections, while a review is active.
    awaiting: Vec<Item>,
    recent: Vec<Item>,
}

fn read_lists(main: &MainWindow, home: &HomeView, cx: &App) -> Lists {
    let tabs = main.tabs();
    let open = tabs
        .items()
        .iter()
        .enumerate()
        .filter_map(|(ix, item)| {
            let tab = item.review()?.read(cx);
            let item = Item {
                review_id: tab.review_id.clone(),
                kind: tab.opened.kind,
                title: tab.title(),
            };
            Some((item, ix == tabs.active()))
        })
        .collect();
    let home_active = tabs.active() == 0;
    let section = |rows: &[HomeRow]| -> Vec<Item> {
        rows.iter()
            .map(|r| Item {
                review_id: r.review_id().to_owned(),
                kind: r.summary.kind,
                title: r.title.clone(),
            })
            .collect()
    };
    // On Home the page shows these lists already (OQ-36).
    let (awaiting, recent) = if home_active {
        (Vec::new(), Vec::new())
    } else {
        (section(home.awaiting_you()), section(home.recent()))
    };
    Lists {
        home_active,
        open,
        awaiting_count: home.awaiting_you().len(),
        awaiting,
        recent,
    }
}

impl RenderOnce for Nav {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let list = v_flex()
            .id("nav")
            .debug_selector(|| "nav".into())
            .size_full()
            .overflow_y_scroll()
            .px_2()
            .pt_1()
            .pb_3()
            .gap_0p5();
        let (Some((_, main)), Some(home)) = (crate::window::main_window(cx), super::home_view(cx))
        else {
            return list;
        };
        let lists = read_lists(main.read(cx), home.read(cx), cx);
        let (main, home) = (main.downgrade(), home.downgrade());

        // Home.
        let theme = cx.theme();
        let count = (lists.awaiting_count > 0).then(|| {
            let n = lists.awaiting_count;
            div()
                .debug_selector(move || format!("nav-home-awaiting-{n}"))
                .flex_none()
                .text_xs()
                .font_medium()
                .text_color(theme.muted_foreground)
                .child(n.to_string())
        });
        let home_row = {
            let main = main.clone();
            row("nav-home".into(), lists.home_active, cx)
                .child(icon(IconName::Inbox, cx))
                .child(label("Home"))
                .children(count)
                .on_click(move |_, window, cx| {
                    main.update(cx, |m, cx| m.activate_tab(0, window, cx)).ok();
                })
        };

        // Open, with Open… in its heading.
        let open_button = Button::new("nav-open")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .tooltip("Open… (⌘O)")
            .debug_selector(|| "nav-open".into())
            .on_click(|_, window, cx| {
                crate::open_flow::open(window, cx);
            });
        let mut children = vec![
            home_row.into_any_element(),
            heading(
                "nav-section-open",
                "Open",
                Some(open_button.into_any_element()),
                cx,
            ),
        ];
        for (item, active) in lists.open {
            children.push(open_row(item, active, &main, cx).into_any_element());
        }
        // While a review is active: Home's two lists.
        for (selector, title, items) in [
            ("nav-section-awaiting", "Awaiting you", lists.awaiting),
            ("nav-section-recent", "Recent", lists.recent),
        ] {
            if items.is_empty() {
                continue;
            }
            children.push(heading(selector, title, None, cx));
            for item in items {
                children.push(compact_row(item, home.clone(), window, cx));
            }
        }
        list.children(children)
    }
}

/// A row of the list, `name` its element id and debug selector.
fn row(name: SharedString, active: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(name.clone())
        .debug_selector(move || name.into())
        .group(ROW_GROUP)
        .flex_none()
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap_2()
        .rounded(theme.radius)
        .text_sm()
        .text_color(theme.sidebar_foreground)
        .when(active, |el| el.bg(theme.list_active))
        .when(!active, |el| el.hover(|s| s.bg(theme.list_hover)))
}

fn icon(name: impl Into<Icon>, cx: &App) -> Icon {
    Icon::new(name)
        .small()
        .text_color(cx.theme().muted_foreground)
}

fn label(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_w_0().truncate().child(text.into())
}

/// The icon of a review's kind (the toolbar pills' icons).
fn kind_icon(kind: ReviewKind) -> Lucide {
    match kind {
        ReviewKind::Live => Lucide::GitBranch,
        ReviewKind::Compare => Lucide::GitCompare,
        ReviewKind::Commit => Lucide::GitCommitHorizontal,
    }
}

/// A section's heading: its title, then `trailing` at the right.
fn heading(
    selector: &'static str,
    title: &'static str,
    trailing: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    h_flex()
        .debug_selector(move || selector.into())
        .flex_none()
        .h(px(ROW_HEIGHT))
        .mt_2()
        .pl_2()
        .pr_1()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(div().flex_1().child(title))
        .children(trailing)
        .into_any_element()
}

/// Shows `child` (a row's × or ⋯) only while the row is hovered, or always
/// when `pinned`. Hidden, it still takes its room, so the row never shifts,
/// and takes no clicks.
fn on_hover(child: impl IntoElement, pinned: bool) -> Div {
    div()
        // The row acts on click; its buttons must not.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(!pinned, |el| {
            el.invisible().group_hover(ROW_GROUP, |s| s.visible())
        })
        .child(child)
}

/// An open review: a click activates it, × closes it as ⌘W does.
fn open_row(item: Item, active: bool, main: &WeakEntity<MainWindow>, cx: &App) -> Stateful<Div> {
    let id = item.review_id;
    // The tab of the row's review when the click runs, if it is still open.
    let tab_ix = {
        let id = id.clone();
        move |m: &MainWindow, cx: &App| m.tabs().find_review(&id, cx)
    };
    let close = {
        let (main, tab_ix, selector) = (main.clone(), tab_ix.clone(), format!("close-review-{id}"));
        Button::new(SharedString::from(format!("close-review-{id}")))
            .icon(IconName::Close)
            .ghost()
            .xsmall()
            .tooltip("Close Review (⌘W)")
            .debug_selector(move || selector)
            .on_click(move |_, window, cx| {
                main.update(cx, |m, cx| {
                    if let Some(ix) = tab_ix(m, cx) {
                        m.close_tab_at(ix, window, cx);
                    }
                })
                .ok();
            })
    };
    let live = (item.kind == ReviewKind::Live).then(|| {
        let selector = format!("nav-live-dot-{id}");
        div()
            .debug_selector(move || selector)
            .flex_none()
            .size(px(6.))
            .rounded_full()
            .bg(cx.theme().success)
    });
    let activate = main.clone();
    row(format!("open-review-{id}").into(), active, cx)
        .child(icon(kind_icon(item.kind), cx))
        .child(label(item.title))
        .children(live)
        .child(on_hover(close, false))
        .on_click(move |_, window, cx| {
            activate
                .update(cx, |m, cx| {
                    if let Some(ix) = tab_ix(m, cx) {
                        m.activate_tab(ix, window, cx);
                    }
                })
                .ok();
        })
}

/// A review of Home's lists: a click opens or focuses it; ⋯ (on hover, or
/// while its menu is open) and a right click show Home's row menu.
fn compact_row(
    item: Item,
    home: WeakEntity<HomeView>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = item.review_id;
    let menu_open = window.use_keyed_state(
        SharedString::from(format!("nav-row-menu-open-{id}")),
        cx,
        |_, _| false,
    );
    let pinned = *menu_open.read(cx);
    let menu = {
        let (home, id, selector) = (home.clone(), id.clone(), format!("nav-row-menu-{id}"));
        Button::new(SharedString::from(format!("nav-row-menu-{id}")))
            .icon(IconName::Ellipsis)
            .ghost()
            .xsmall()
            .tooltip("Actions")
            .debug_selector(move || selector)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                HomeView::menu_for(&home, &id, menu, window, cx)
            })
            .on_open_change(move |open, _, cx| {
                menu_open.update(cx, |o, cx| {
                    *o = *open;
                    cx.notify();
                })
            })
    };
    let (open_home, open_id) = (home.clone(), id.clone());
    let ctx_id = id.clone();
    row(format!("nav-row-{id}").into(), false, cx)
        .child(icon(kind_icon(item.kind), cx))
        .child(label(item.title))
        .child(on_hover(menu, pinned))
        .on_click(move |_, window, cx| {
            open_home
                .update(cx, |h, cx| h.open_row(&open_id, window, cx))
                .ok();
        })
        .context_menu(move |menu, window, cx| HomeView::menu_for(&home, &ctx_id, menu, window, cx))
        .into_any_element()
}
