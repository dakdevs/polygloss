//! The command palette (⌘K, design §11.8): every action of the registry
//! with its current key binding, filtered as you type (gpui-kit `Command`).
//! Choosing one closes the palette and dispatches the action where focus
//! was before it opened, so it acts on that tab exactly as its key would.

use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Global, IntoElement, Keystroke,
    ParentElement as _, Render, SharedString, Styled as _, WeakEntity, Window, div, px,
};

use crate::keymap::KeymapStore;
use crate::keymap::actions::{ACTIONS, ActionInfo};
use crate::palette::key_cap::{key_cap, key_label};

/// One row of the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteRow {
    /// The action's registry name (`viewport::ToggleLayout`).
    pub action: &'static str,
    pub title: &'static str,
    /// The binding shown, as GPUI keys (`s`, `cmd-k`).
    pub keys: Option<String>,
}

impl PaletteRow {
    /// The binding as the row shows it (`S`, `⌘K`).
    pub fn hint(&self) -> Option<String> {
        self.keys.as_deref().map(format_keys)
    }
}

/// `keys` as macOS shows shortcuts (`cmd-shift-enter` → `⌘⇧⏎`… as gpui-kit's
/// `Kbd` spells them, Escape as `Esc`); strokes of a sequence are separated
/// by spaces.
pub fn format_keys(keys: &str) -> String {
    keys.split_whitespace()
        .filter_map(|k| Keystroke::parse(k).ok())
        .map(|k| key_label(&k))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A palette group's heading for an action namespace.
pub fn group_title(namespace: &str) -> &'static str {
    match namespace {
        "viewport" => "Diff",
        "tree" => "File tree",
        "composer" => "Comment",
        "tab" => "Review",
        "window" => "Window",
        _ => "Other",
    }
}

/// The palette's groups: every registry action, by namespace, with the
/// binding in effect.
pub fn groups(cx: &App) -> Vec<(&'static str, Vec<PaletteRow>)> {
    let resolved = KeymapStore::global(cx).resolved();
    let mut out: Vec<(&'static str, Vec<PaletteRow>)> = Vec::new();
    for info in ACTIONS {
        let row = PaletteRow {
            action: info.name,
            title: info.title,
            keys: resolved.hint_for(info.name).map(|b| b.keys.clone()),
        };
        let heading = group_title(info.namespace());
        match out.iter_mut().find(|(h, _)| *h == heading) {
            Some((_, rows)) => rows.push(row),
            None => out.push((heading, vec![row])),
        }
    }
    out
}

/// The open palette (a dialog's content).
pub struct CommandPalette {
    state: Entity<CommandState>,
    /// Where focus was when the palette opened: the chosen action is
    /// dispatched there.
    target: Option<FocusHandle>,
    groups: Vec<(&'static str, Vec<PaletteRow>)>,
}

/// The palette while it is open (tests and the cheat sheet read it).
#[derive(Default)]
struct OpenPalette(Option<WeakEntity<CommandPalette>>);

impl Global for OpenPalette {}

/// Opens the palette over `window`, its search field focused.
pub fn open(window: &mut Window, cx: &mut App) -> Entity<CommandPalette> {
    let target = window.focused(cx);
    let groups = groups(cx);
    let palette = cx.new(|cx| CommandPalette {
        state: cx.new(|cx| CommandState::new(window, cx)),
        target,
        groups,
    });
    cx.set_global(OpenPalette(Some(palette.downgrade())));
    let content = palette.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(560.))
            .margin_top(px(96.))
            .close_button(false)
            .child(content.clone())
    });
    let state = palette.read(cx).state.clone();
    state.update(cx, |s, cx| s.focus(window, cx));
    palette
}

/// The open palette, if any.
pub fn current(cx: &App) -> Option<Entity<CommandPalette>> {
    cx.try_global::<OpenPalette>()?.0.as_ref()?.upgrade()
}

impl CommandPalette {
    /// The rows, by group.
    pub fn groups(&self) -> &[(&'static str, Vec<PaletteRow>)] {
        &self.groups
    }

    /// Every row, in order.
    pub fn rows(&self) -> impl Iterator<Item = &PaletteRow> {
        self.groups.iter().flat_map(|(_, rows)| rows)
    }

    /// The search field and list state.
    pub fn state(&self) -> &Entity<CommandState> {
        &self.state
    }
}

/// Runs the row at `ix` (group, row) of `palette`: closes it and dispatches
/// the row's action where focus was when it opened.
pub fn confirm(palette: &Entity<CommandPalette>, ix: IndexPath, window: &mut Window, cx: &mut App) {
    let p = palette.read(cx);
    let target = p.target.clone();
    let Some(info) = p
        .groups
        .get(ix.section)
        .and_then(|(_, rows)| rows.get(ix.row))
        .and_then(|row| crate::keymap::actions::find(row.action))
    else {
        return;
    };
    run(info, target, window, cx);
}

/// Closes the palette and dispatches `info`'s action on `target`.
fn run(info: &ActionInfo, target: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
    cx.set_global(OpenPalette(None));
    let action = (info.build)();
    match target {
        Some(target) => {
            window.focus(&target, cx);
            target.dispatch_action(action.as_ref(), window, cx);
        }
        None => window.dispatch_action(action, cx),
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let mut command = Command::new(&self.state)
            .bordered(false)
            .max_h(px(380.))
            .placeholder("Type a command…")
            .empty(|_, _, cx| {
                div()
                    .py_6()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No matching commands")
            })
            .on_confirm(move |ix, window, cx| {
                if let Some(this) = this.upgrade() {
                    confirm(&this, ix, window, cx);
                }
            });
        for (heading, rows) in &self.groups {
            let items = rows.iter().map(|row| {
                let title: SharedString = row.title.into();
                let keys = row.keys.clone();
                CommandItem::new().label(title.clone()).child(move |_, cx| {
                    let muted = cx.theme().muted_foreground;
                    h_flex()
                        .flex_1()
                        .gap_2()
                        .items_center()
                        .justify_between()
                        .child(div().truncate().child(title.clone()))
                        .when_some(keys.clone(), |row, keys| {
                            row.child(
                                h_flex().flex_none().gap_1().text_color(muted).children(
                                    keys.split_whitespace()
                                        .filter_map(|k| Keystroke::parse(k).ok())
                                        .map(|k| key_cap(&k, cx)),
                                ),
                            )
                        })
                })
            });
            command = command.group(CommandGroup::new().label(*heading).items(items));
        }
        command
    }
}
