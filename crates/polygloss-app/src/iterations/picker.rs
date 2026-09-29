//! The toolbar's iteration picker (design §11.4): an outline dropdown
//! button labeled "Iteration k of n" (or "Working tree", or "Changes since
//! last review", highlighted, when that mode is on). Its menu, like
//! GitHub's commit picker, starts with the **Changes since last review**
//! toggle (checked when on; disabled, with the reason as its note, when
//! there is no submission or nothing changed since), then lists the states
//! newest first with what each one is and when it was pinned.

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, Window, div, px,
};
use polygloss_core::store::events::now_ms;

use super::{
    PickerEntry, changes_since_available, changes_since_checked, changes_since_hint, label,
    picker_entries, picker_visible, show, toggle_changes_since,
};
use crate::home::row::{local_utc_offset_s, relative_time};
use crate::review_tab::ReviewTab;

/// The picker, when the review has something to pick (see
/// [`picker_visible`]).
pub fn toolbar_items(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    if !picker_visible(tab) {
        return Vec::new();
    }
    let this = cx.entity().downgrade();
    let since_on = changes_since_checked(tab);
    let button = Button::new("iteration-picker")
        .label(label(tab))
        .dropdown_caret(true)
        .xsmall()
        .outline()
        .selected(since_on)
        .tooltip("Choose which iteration to show")
        .dropdown_menu(move |mut menu, _, cx| {
            let Some(tab) = this.upgrade() else {
                return menu;
            };
            let (entries, available, checked, hint) = {
                let t = tab.read(cx);
                (
                    picker_entries(t),
                    changes_since_available(t),
                    changes_since_checked(t),
                    changes_since_hint(t),
                )
            };
            let toggle = {
                let tab = tab.downgrade();
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
                let tab = tab.downgrade();
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
        });
    vec![
        div()
            .debug_selector(|| "iteration-picker".into())
            .child(button)
            .into_any_element(),
    ]
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
