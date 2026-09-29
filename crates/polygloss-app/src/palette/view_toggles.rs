//! The view toggles of a review tab (design §11.4, §11.6, §11.9): split /
//! unified (`s`, remembered per diff), hide whitespace (`w`) and word diff
//! by words, characters or off.
//!
//! They are per-tab overrides on top of the settings ([`ViewOverrides`], a
//! tab extension): a settings reload rebuilds the viewport options from the
//! settings and applies them again (`ReviewTab::options_for`). The manual
//! layout is remembered per `diff_id` for the session ([`LayoutChoices`])
//! and in the store's view state (`view_state.layout`, T1.14), so reopening
//! the diff shows it again; a live diff not pinned yet has no view state and
//! is remembered for the session only. T3.14 saves and restores the rest of
//! the view state through [`layout_choice`] / [`set_layout_choice`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui_kit::{App, AppContext as _, Context, Global, Window};
use polygloss_core::review::{Core, CoreError};
use polygloss_diff::rows::Layout;
use polygloss_diff::word::Granularity;
use polygloss_viewport::{LayoutMode, ViewportOptions};

use crate::app_state::AppState;
use crate::keymap::actions::viewport;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::settings::SettingsStore;

/// A tab's view choices over the settings (`None`: the setting applies).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewOverrides {
    pub layout: Option<LayoutMode>,
    pub hide_whitespace: Option<bool>,
    /// `Some(None)` turns word diff off.
    pub word_diff: Option<Option<Granularity>>,
}

impl ViewOverrides {
    /// `opts` with these choices applied.
    pub fn apply(&self, opts: &mut ViewportOptions) {
        if let Some(layout) = self.layout {
            opts.layout = layout;
        }
        if let Some(hide) = self.hide_whitespace {
            opts.diff.ignore_whitespace = hide;
        }
        if let Some(word_diff) = self.word_diff {
            opts.word_diff = word_diff;
        }
    }
}

/// The per-tab state (a [`ReviewTab`] extension).
struct Toggles {
    overrides: ViewOverrides,
    /// Serializes the layout's read-modify-write of the stored view state,
    /// holding the latest choice.
    persisted: Arc<Mutex<Option<LayoutMode>>>,
}

/// Manual layouts chosen this session, by `diff_id` (`Auto` included: the
/// user went back to automatic).
#[derive(Default)]
pub struct LayoutChoices(HashMap<String, LayoutMode>);

impl Global for LayoutChoices {}

/// Registers the toggle actions' handlers on review tabs.
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::ToggleLayout, _, cx| {
            let next = match tab.viewport.read(cx).effective_layout() {
                Layout::Split => LayoutMode::Unified,
                Layout::Unified => LayoutMode::Split,
            };
            set_layout_choice(tab, next, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::LayoutSplit, _, cx| {
            set_layout_choice(tab, LayoutMode::Split, cx)
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::LayoutUnified, _, cx| {
            set_layout_choice(tab, LayoutMode::Unified, cx)
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::LayoutAuto, _, cx| {
            set_layout_choice(tab, LayoutMode::Auto, cx)
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::ToggleWhitespace, _, cx| {
            let hidden = tab.viewport.read(cx).options().diff.ignore_whitespace;
            update(tab, cx, |o| o.hide_whitespace = Some(!hidden));
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::WordDiffWord, _, cx| {
            update(tab, cx, |o| o.word_diff = Some(Some(Granularity::Word)))
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::WordDiffChar, _, cx| {
            update(tab, cx, |o| o.word_diff = Some(Some(Granularity::Char)))
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::WordDiffOff, _, cx| {
            update(tab, cx, |o| o.word_diff = Some(None))
        },
    );
}

/// Sets up the toggles of a new tab: the diff's remembered layout (this
/// session's choice, else the stored view state's) applies before the first
/// frame.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    let diff_id = tab.opened.diff_id.as_str().to_owned();
    let remembered = cx
        .try_global::<LayoutChoices>()
        .and_then(|c| c.0.get(&diff_id).copied())
        .or_else(|| stored_layout(&AppState::global(cx).core, &tab.opened.diff_id));
    tab.insert_extension(Toggles {
        overrides: ViewOverrides {
            layout: remembered,
            ..ViewOverrides::default()
        },
        persisted: Arc::new(Mutex::new(remembered)),
    });
    if remembered.is_some() {
        refresh(tab, cx);
    }
}

/// The layout stored in `diff_id`'s view state (`None`: no manual choice).
fn stored_layout(core: &Core, diff_id: &polygloss_core::ids::DiffId) -> Option<LayoutMode> {
    match core.load_view_state(diff_id) {
        Ok(state) => state.and_then(|s| s.layout).map(|l| match l {
            Layout::Split => LayoutMode::Split,
            Layout::Unified => LayoutMode::Unified,
        }),
        Err(e) => {
            tracing::warn!("reading the view state of {diff_id}: {e}");
            None
        }
    }
}

/// This tab's choices.
pub fn overrides(tab: &ReviewTab) -> ViewOverrides {
    tab.extension::<Toggles>()
        .map(|t| t.overrides)
        .unwrap_or_default()
}

/// `opts` with the tab's choices applied (`ReviewTab::options_for`).
pub fn apply_overrides(tab: &ReviewTab, opts: &mut ViewportOptions) {
    overrides(tab).apply(opts);
}

/// The tab's manual layout (`None`: none this session or stored).
pub fn layout_choice(tab: &ReviewTab) -> Option<LayoutMode> {
    overrides(tab).layout
}

/// Chooses the tab's layout and remembers it for the diff: for the session,
/// and in the stored view state (`Auto` clears it there), off the main
/// thread. The scroll anchor stays.
pub fn set_layout_choice(tab: &mut ReviewTab, layout: LayoutMode, cx: &mut Context<ReviewTab>) {
    update(tab, cx, |o| o.layout = Some(layout));
    let diff_id = tab.opened.diff_id.clone();
    cx.default_global::<LayoutChoices>()
        .0
        .insert(diff_id.as_str().to_owned(), layout);
    let Some(toggles) = tab.extension::<Toggles>() else {
        return;
    };
    let persisted = toggles.persisted.clone();
    *persisted.lock().unwrap_or_else(|e| e.into_inner()) = Some(layout);
    let core = AppState::global(cx).core.clone();
    cx.background_spawn(async move {
        // Held for the whole read-modify-write, so the last choice wins
        // whatever order these tasks run in.
        let latest = persisted.lock().unwrap_or_else(|e| e.into_inner());
        let layout = match *latest {
            Some(LayoutMode::Split) => Some(Layout::Split),
            Some(LayoutMode::Unified) => Some(Layout::Unified),
            Some(LayoutMode::Auto) | None => None,
        };
        let result = core.load_view_state(&diff_id).and_then(|state| {
            let mut state = state.unwrap_or_default();
            if state.layout == layout {
                return Ok(());
            }
            state.layout = layout;
            core.save_view_state(&diff_id, &state)
        });
        match result {
            Ok(()) => {}
            // A live state not pinned yet: remembered for the session only.
            Err(CoreError::NotFound { .. }) => {}
            Err(e) => tracing::warn!("saving the layout of {diff_id}: {e}"),
        }
    })
    .detach();
}

/// Changes the tab's choices and applies them to its viewport.
fn update(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>, f: impl FnOnce(&mut ViewOverrides)) {
    if tab.extension::<Toggles>().is_none() {
        tab.insert_extension(Toggles {
            overrides: ViewOverrides::default(),
            persisted: Arc::new(Mutex::new(None)),
        });
    }
    if let Some(toggles) = tab.extension_mut::<Toggles>() {
        f(&mut toggles.overrides);
    }
    refresh(tab, cx);
}

/// Rebuilds the viewport's options from the settings and the tab's choices.
fn refresh(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let settings = SettingsStore::global(cx).shared();
    let opts = tab.options_for(&settings, cx);
    tab.viewport.update(cx, |v, cx| v.set_options(opts, cx));
    cx.notify();
}
