//! The sidebar's Reviews segment (design §11.2, OQ-36): a Home row with the
//! awaiting count; **Open**, with Open… (⌘O) in its heading and one row per
//! open review (kind icon, `repo · summary`, a live dot, × on hover, the
//! active one highlighted); and while a review is active, Home's **Awaiting
//! you** and **Recent** as compact rows with Home's ⋯ and context menus
//! ([`HomeView::row_menu`]). A click opens or focuses the row's review; ×
//! closes it as ⌘W does. The list is for the mouse: the keyboard has the Home
//! page (⌘0) and ⌘1–⌘9.
//!
//! Spacing (ADR-0031 S1–S4): the list sits on the sidebar's edge, its first
//! row `RIM` below the top row as the filter field is; rows are `MD` row
//! highlights that touch, their icon box `ICON_LEAD` in and the label
//! `ICON_LABEL` after it; a trailing button's icon ends `ICON_LEAD` from the
//! highlight's end and trailing text `TEXT`; headings are plain text on the
//! rows' text column, `CONTROLS` below the row before them. Rows show press
//! ink.
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
use crate::space::{TextStyleExt as _, edge, gap, height, pad, radius, size, text};
use crate::window::MainWindow;

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
            .px(px(edge::SIDEBAR))
            .pt(px(pad::RIM))
            .pb(px(edge::SIDEBAR));
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
                .text_style(text::SMALL)
                .font_features(crate::chrome::tabular_figures())
                .font_medium()
                .text_color(theme.muted_foreground)
                .child(n.to_string())
        });
        let home_row = {
            let main = main.clone();
            row("nav-home".into(), lists.home_active, cx)
                .pr(px(pad::TEXT))
                .child(icon("nav-home", IconName::Inbox, cx))
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
            .rounded(px(radius::XS))
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

/// A row of the list, `name` its element id and debug selector, ending in
/// a trailing button (callers ending in text set `pad::TEXT`).
fn row(name: SharedString, active: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let radius = radius::for_height(height::MD);
    h_flex()
        .id(name.clone())
        .debug_selector(move || name.into())
        .group(ROW_GROUP)
        .relative()
        .flex_none()
        .h(px(height::MD))
        .pl(px(pad::ICON_LEAD))
        .pr(px(button_end()))
        .gap(px(gap::ICON_LABEL))
        .rounded(px(radius))
        .text_style(text::UI)
        .text_color(theme.sidebar_foreground)
        .when(active, |el| el.bg(theme.list_active))
        .child(crate::chrome::ink_layer(radius, cx))
}

/// From a row's or heading's end to its trailing `.xsmall()` button, so
/// the button's icon ends `ICON_LEAD` from the end, as leading icons start.
fn button_end() -> f32 {
    pad::ICON_LEAD - (height::XS - size::ICON_XS) / 2.0
}

/// A row's icon (`ICON_SM`), found as `<row>-icon`.
fn icon(row: &str, name: impl Into<Icon>, cx: &App) -> Div {
    let selector = format!("{row}-icon");
    div()
        .debug_selector(move || selector)
        .flex_none()
        .flex()
        .child(
            Icon::new(name)
                .with_size(px(size::ICON_SM))
                .text_color(cx.theme().muted_foreground),
        )
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
        .h(px(height::MD))
        .mt(px(gap::CONTROLS))
        .pl(px(pad::TEXT))
        .pr(px(button_end()))
        .text_style(text::SMALL)
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
            .rounded(px(radius::XS))
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
            .size(px(size::DOT))
            .rounded_full()
            .bg(cx.theme().success)
    });
    let activate = main.clone();
    let name = format!("open-review-{id}");
    row(name.clone().into(), active, cx)
        .child(icon(&name, kind_icon(item.kind), cx))
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
            .rounded(px(radius::XS))
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
    let name = format!("nav-row-{id}");
    row(name.clone().into(), false, cx)
        .child(icon(&name, kind_icon(item.kind), cx))
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
