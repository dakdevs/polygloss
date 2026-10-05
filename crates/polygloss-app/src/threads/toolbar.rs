//! The threads in the toolbar (design §11.4): the threads button, which
//! counts open threads and shows or hides the threads panel, and the display
//! options menu's "Hide agent notes" / "Show agent notes (count)".

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, Role, StatefulInteractiveElement as _, Styled as _, Toggled, Window, div,
    px,
};
use polygloss_core::review::{ThreadKind, ThreadStatus};

use super::threads;
use crate::keymap::actions::tab as tab_actions;
use crate::palette::MenuEntry;
use crate::review_tab::ReviewTab;
use crate::review_tab::toolbar::{text, tooltip};

/// `(open threads, agent notes)` of the tab: open threads that wait on
/// someone (notes are FYI and counted apart, as in the threads panel), and
/// every agent note.
pub fn open_counts(tab: &ReviewTab, cx: &App) -> (u32, u32) {
    let Some(model) = threads(tab) else {
        return (0, 0);
    };
    let (mut open, mut notes) = (0, 0);
    for t in model.read(cx).threads() {
        if t.kind == ThreadKind::Note {
            notes += 1;
        } else if t.status == ThreadStatus::Open {
            open += 1;
        }
    }
    (open, notes)
}

/// The toolbar's threads button (design §11.4): the message icon and the
/// open threads; selected while the panel shows; a click shows or hides
/// the panel. While agent notes are hidden it carries a muted dot and its
/// tooltip says how many, so they are never out of sight unannounced.
pub fn toolbar_button(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let (open, notes) = open_counts(tab, cx);
    let hidden = notes > 0 && threads(tab).is_some_and(|m| m.read(cx).hide_agent_notes());
    let shown = tab.threads_panel_visible();
    let mut tip = if shown {
        "Hide threads panel"
    } else {
        "Show threads panel"
    }
    .to_owned();
    if hidden {
        tip.push_str(&match notes {
            1 => " · 1 agent note hidden".to_owned(),
            n => format!(" · {n} agent notes hidden"),
        });
    }
    let theme = cx.theme();
    let icon = if hidden {
        div()
            .debug_selector(|| "threads-notes-hidden".into())
            .child(Icon::new(Lucide::MessageSquareDot).small())
    } else {
        div().child(Icon::new(Lucide::MessageSquare).small())
    };
    let (selected, hover) = (theme.list_active, theme.list_hover);
    h_flex()
        .id("toggle-threads-panel")
        .debug_selector(|| "toggle-threads-panel".into())
        .flex_none()
        .h(px(24.))
        .px_1p5()
        .gap_1()
        .rounded(px(6.))
        .text_sm()
        .map(|button| {
            if shown {
                button.bg(selected).text_color(theme.foreground)
            } else {
                button
                    .text_color(theme.muted_foreground)
                    .hover(move |s| s.bg(hover))
            }
        })
        // A control: a press on it never moves the window.
        .role(Role::Button)
        .aria_label("Threads panel")
        .aria_toggled(if shown { Toggled::True } else { Toggled::False })
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        // Straight to the tab, not an action dispatched from the focused
        // element: the click works wherever focus is.
        .on_click(
            cx.listener(|tab, _, window, cx| tab.toggle_threads_panel_from_toolbar(window, cx)),
        )
        .child(icon)
        .child(text("threads-count", open.to_string()))
        .tooltip(tooltip(tip))
        .into_any_element()
}

/// "Hide agent notes" / "Show agent notes (count)" for the display options
/// menu (design §11.4), when the review has agent notes.
pub fn agent_notes_entry(tab: &ReviewTab, cx: &App) -> Option<MenuEntry> {
    let (_, notes) = open_counts(tab, cx);
    if notes == 0 {
        return None;
    }
    let hidden = threads(tab)?.read(cx).hide_agent_notes();
    let label = if hidden {
        format!("Show agent notes ({notes})")
    } else {
        "Hide agent notes".to_owned()
    };
    Some(MenuEntry::action(label, tab_actions::ToggleAgentNotes))
}
