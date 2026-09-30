//! The keyboard shortcuts cheat sheet (`?`, design §11.8): every binding in
//! effect, `keymap.json` included, grouped by where it works.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, IntoElement, Keystroke, ParentElement as _,
    Render, Styled as _, WeakEntity, Window, div, px,
};

use crate::keymap::{KeymapStore, Resolved, actions};
use crate::palette::key_cap::key_cap;

/// One row: an action and every key bound to it in the section's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatRow {
    pub action: &'static str,
    pub title: &'static str,
    /// GPUI keys (`j`, `down`).
    pub keys: Vec<String>,
}

/// The sections, in order: `(heading, contexts they cover)`.
const SECTIONS: &[(&str, &[&str])] = &[
    ("Diff", &["Viewport"]),
    ("File tree", &["Tree"]),
    ("Comment", &["Composer"]),
    ("Threads", &["ThreadsPanel"]),
    ("Review", &["Tab"]),
    ("Window", &["Window", ""]),
];

/// The cheat sheet's sections: `(heading, rows)`, empty sections left out.
/// Bindings of other contexts (a `keymap.json` may name any) go under
/// "Other".
pub fn sections(resolved: &Resolved) -> Vec<(&'static str, Vec<CheatRow>)> {
    let mut out: Vec<(&'static str, Vec<CheatRow>)> =
        SECTIONS.iter().map(|(h, _)| (*h, Vec::new())).collect();
    out.push(("Other", Vec::new()));
    for binding in &resolved.bindings {
        if !resolved
            .bindings_for(binding.action)
            .iter()
            .any(|b| std::ptr::eq(*b, binding))
        {
            continue;
        }
        let context = binding.context.as_deref().unwrap_or("");
        let section = SECTIONS
            .iter()
            .position(|(_, contexts)| contexts.contains(&context))
            .unwrap_or(SECTIONS.len());
        let rows = &mut out[section].1;
        match rows.iter_mut().find(|r| r.action == binding.action) {
            Some(row) => row.keys.push(binding.keys.clone()),
            None => rows.push(CheatRow {
                action: binding.action,
                title: actions::find(binding.action).map_or(binding.action, |a| a.title),
                keys: vec![binding.keys.clone()],
            }),
        }
    }
    out.retain(|(_, rows)| !rows.is_empty());
    out
}

/// The open cheat sheet (a dialog's content).
pub struct CheatSheet {
    sections: Vec<(&'static str, Vec<CheatRow>)>,
}

#[derive(Default)]
struct OpenCheatSheet(Option<WeakEntity<CheatSheet>>);

impl Global for OpenCheatSheet {}

/// Opens the cheat sheet over `window` (Esc closes it).
pub fn open(window: &mut Window, cx: &mut App) -> Entity<CheatSheet> {
    let sheet = cx.new(|cx| CheatSheet {
        sections: sections(KeymapStore::global(cx).resolved()),
    });
    cx.set_global(OpenCheatSheet(Some(sheet.downgrade())));
    let content = sheet.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(1040.))
            .margin_top(px(72.))
            .title("Keyboard Shortcuts")
            .child(content.clone())
    });
    sheet
}

/// The open cheat sheet, if any.
pub fn current(cx: &App) -> Option<Entity<CheatSheet>> {
    cx.try_global::<OpenCheatSheet>()?.0.as_ref()?.upgrade()
}

impl CheatSheet {
    pub fn sections(&self) -> &[(&'static str, Vec<CheatRow>)] {
        &self.sections
    }
}

fn section(heading: &'static str, rows: &[CheatRow], cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(
            div()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(heading.to_uppercase()),
        )
        .children(rows.iter().map(|row| {
            h_flex()
                .h(px(26.))
                .gap_3()
                .justify_between()
                .text_sm()
                .child(div().truncate().child(row.title))
                .child(
                    h_flex().flex_none().gap_1().children(
                        row.keys
                            .iter()
                            .filter_map(|k| {
                                k.split_whitespace()
                                    .next()
                                    .and_then(|k| Keystroke::parse(k).ok())
                            })
                            .map(|k| key_cap(&k, cx)),
                    ),
                )
        }))
}

impl Render for CheatSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Three columns: the diff's keys; the file tree's, the composer's
        // and the threads panel's; the review's and the window's (and any
        // other context's).
        let side = |h: &str| matches!(h, "File tree" | "Comment" | "Threads");
        let left: Vec<_> = self.sections.iter().take(1).cloned().collect();
        let (middle, right): (Vec<_>, Vec<_>) = self
            .sections
            .iter()
            .skip(1)
            .cloned()
            .partition(|(h, _)| side(h));
        let column = |sections: &[(&'static str, Vec<CheatRow>)], cx: &App| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_5()
                .children(sections.iter().map(|(h, rows)| section(h, rows, cx)))
        };
        h_flex()
            .items_start()
            .gap_8()
            .pb_2()
            .child(column(&left, cx))
            .child(column(&middle, cx))
            .child(column(&right, cx))
    }
}
