//! The toolbar's iteration pill (design §11.4): "Iteration k of n" (or
//! "Working tree", or "Changes since last review", selected, when that mode
//! is on), shortened to "k/n" or "Since review" when the toolbar is narrow
//! and to its icon after that. Its menu, like GitHub's commit picker,
//! starts with the **Changes since last review** toggle (checked when on;
//! disabled, with the reason as its note, when there is no submission or
//! nothing changed since), then lists the states newest first with what
//! each one is and when it was pinned. `i` opens the same menu under it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    WeakEntity, Window, div, px,
};
use polygloss_core::store::events::now_ms;

use super::{
    PickerEntry, Showing, changes_since_available, changes_since_checked, changes_since_hint,
    label, picker_entries, picker_visible, show, state, toggle_changes_since,
};
use crate::home::row::{local_utc_offset_s, relative_time};
use crate::review_tab::ReviewTab;
use crate::review_tab::toolbar::{self, Narrow};

/// [`label`] for a narrow toolbar: "k/n", "Since review" (a working tree
/// not pinned stays "Working tree").
pub fn short_label(tab: &ReviewTab) -> String {
    let Some(s) = state(tab) else {
        return String::new();
    };
    let n = s.count();
    match s.showing {
        Showing::ChangesSince { .. } => "Since review".to_owned(),
        Showing::Iteration(k) => format!("{k}/{n}"),
        Showing::Current => match s.current_seq() {
            Some(k) => format!("{k}/{n}"),
            None => "Working tree".to_owned(),
        },
    }
}

/// The iteration pill, when the review has something to pick (see
/// [`picker_visible`]), with the keyboard's menu (`i`) hanging from it.
pub fn toolbar_left(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    if !picker_visible(tab) {
        return None;
    }
    let narrow = toolbar::narrow(tab);
    let full = label(tab);
    let text = (narrow < Narrow::IconPills).then(|| {
        if narrow >= Narrow::ShortIteration {
            short_label(tab)
        } else {
            full.clone()
        }
    });
    let tip = match text {
        Some(_) => "Choose which iteration to show".to_owned(),
        None => full,
    };
    let this = cx.entity().downgrade();
    let pill = toolbar::pill(
        "iteration-picker",
        Lucide::Layers,
        text.map(|t| toolbar::pill_text("iteration-picker-label", t)),
        cx,
    )
    .selected(changes_since_checked(tab))
    .tooltip(tip)
    .dropdown_menu(move |menu, _, cx| match this.upgrade() {
        Some(tab) => {
            let data = MenuData::of(tab.read(cx));
            build_menu(&this, data, menu)
        }
        None => menu,
    });
    let key_menu = state(tab).and_then(|s| s.key_menu.as_ref());
    Some(
        div()
            .flex_none()
            .relative()
            .child(pill)
            .when_some(key_menu, |el, menu| el.child(menu.element(Anchor::TopLeft)))
            .into_any_element(),
    )
}

/// What the menu lists, read as it opens.
pub(crate) struct MenuData {
    entries: Vec<PickerEntry>,
    available: bool,
    checked: bool,
    hint: String,
}

impl MenuData {
    pub(crate) fn of(t: &ReviewTab) -> MenuData {
        MenuData {
            entries: picker_entries(t),
            available: changes_since_available(t),
            checked: changes_since_checked(t),
            hint: changes_since_hint(t),
        }
    }
}

/// The picker's menu (its button's, and `i`'s): the **Changes since last
/// review** toggle, then the states newest first.
pub(crate) fn build_menu(
    tab: &WeakEntity<ReviewTab>,
    data: MenuData,
    mut menu: PopupMenu,
) -> PopupMenu {
    let MenuData {
        entries,
        available,
        checked,
        hint,
    } = data;
    let toggle = {
        let tab = tab.clone();
        PopupMenuItem::element(move |_, cx| {
            two_lines("Changes since last review", hint.clone(), cx)
        })
        .checked(checked)
        .disabled(!available && !checked)
        .on_click(move |_, window, cx| {
            if let Some(tab) = tab.upgrade() {
                tab.update(cx, |t, cx| toggle_changes_since(t, window, cx));
            }
        })
    };
    menu = menu.item(toggle).separator().label("Iterations");
    let now = now_ms();
    let offset = local_utc_offset_s(now);
    for entry in entries {
        let PickerEntry {
            choice,
            label,
            detail,
            at,
            selected,
        } = entry;
        let note = match at {
            Some(at) => format!("{detail} · {}", relative_time(now, at, offset)),
            None => detail,
        };
        let tab = tab.clone();
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| two_lines(&label, note.clone(), cx))
                .checked(selected)
                .on_click(move |_, window, cx| {
                    if let Some(tab) = tab.upgrade() {
                        tab.update(cx, |t, cx| show(t, choice, window, cx));
                    }
                }),
        );
    }
    menu.min_w(px(280.)).max_h(px(420.)).scrollable(true)
}

/// A menu row: a title and a muted note below it.
fn two_lines(title: &str, note: String, cx: &gpui_kit::App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .gap_0p5()
        .py_0p5()
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .child(SharedString::from(title.to_owned())),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(note)),
        )
        .into_any_element()
}
