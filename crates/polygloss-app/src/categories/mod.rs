//! File categories in the app (design §11.15, ADR-0028): which files of a
//! review leave the main list for a closed section at the bottom.
//!
//! - **The categorizer:** [`shared`] compiles `settings.json`'s `categories`
//!   and `diff.generated_patterns` once per settings load, for every tab.
//!   Every provider is built with it ([`crate::CoreDiffProvider::open`]), so
//!   each file's `generated` flag ("Load diff") is its verdict from the
//!   start and opening a review never swaps providers.
//! - **The partition:** [`Partition`] splits a tab's files into the main
//!   list and one section per enabled category. A tab is partitioned
//!   synchronously when it attaches (before its first frame; paths only),
//!   and again synchronously after a refresh or iteration switch
//!   ([`DiffRefreshed`]); a settings change or a palette toggle partitions on
//!   the background executor and only the latest one is applied. The
//!   viewport gets the sections ([`DiffViewport::set_sections`]) and, for
//!   files whose Generated verdict changed, [`DiffViewport::set_generated`];
//!   the tab then emits [`Repartitioned`].
//! - **Open state**, kept per category for the tab ([`Categories`]): the
//!   saved `open_sections` of the diff ([`apply_open_sections`], from view
//!   state), else open when every file is categorized, else closed. Once,
//!   when the tab's threads first load, a section holding an agent question
//!   waiting on you opens, unless a saved state exists or the user opened or
//!   closed a section (OQ-53). Later partitions keep each category's state.
//! - **Palette** (namespace `categories`): Toggle Tests and the other
//!   built-ins (a tab's own on/off, winning over `settings.json`), Show Next
//!   Section, Hide Section, Mark Section Viewed and Explain file category.
//! - **Totals** (T6.15, [`totals`]): the tree's footer and the header card
//!   count the main list's files ([`Totals`]) and add one [`Chip`] per
//!   section ("6 tests"); their tooltip is the [`Breakdown`]
//!   ([`breakdown_lines`]).
//!
//! [`DiffViewport::set_sections`]: polygloss_viewport::DiffViewport::set_sections
//! [`DiffViewport::set_generated`]: polygloss_viewport::DiffViewport::set_generated

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task, Window};
use polygloss_core::categories::{
    BuiltinCategory, CategoriesConfig, Categorizer, CategoryId, CategoryInfo, Explain, Source,
    unknown_groups, unknown_keys,
};
use polygloss_core::review::ViewedState;
use polygloss_diff::FileChange;
use polygloss_viewport::{DiffViewport, Section, ViewportEvent};
use serde_json::{Value, json};

mod totals;

pub use totals::{Breakdown, Chip, Totals, breakdown, breakdown_lines, chips};
pub(crate) use totals::{breakdown_tooltip, chips_element, chips_text};

use crate::keymap::actions::categories as actions;
use crate::keymap::handlers;
use crate::live::DiffRefreshed;
use crate::review_tab::ReviewTab;
use crate::settings::{SETTINGS_FILE, Settings, SettingsStore};

/// A tab's files split by category.
pub struct Partition {
    /// What decided (the shared categorizer, or the tab's own with its
    /// palette toggles).
    pub categorizer: Arc<Categorizer>,
    /// Each file's category, by `file_idx` (`None`: the main list).
    pub category: Vec<Option<CategoryId>>,
    /// The files of the main list, in git order.
    pub main: Vec<u32>,
    /// One per enabled category with files, in match order.
    pub sections: Vec<SectionInfo>,
}

/// A category's section.
pub struct SectionInfo {
    pub id: CategoryId,
    pub info: CategoryInfo,
    /// Its id in the viewport ([`polygloss_viewport::Section::id`]).
    pub viewport_id: u32,
    /// Its files, in git order.
    pub files: Vec<u32>,
}

impl Partition {
    /// Partitions `files` (as stored: the attribute and v1 bit recover
    /// from them) with `categorizer`.
    pub fn new(categorizer: Arc<Categorizer>, files: &[FileChange]) -> Partition {
        let category = categorizer.categorize_files(files);
        let mut sections: Vec<SectionInfo> = categorizer
            .enabled()
            .iter()
            .enumerate()
            .map(|(i, info)| SectionInfo {
                id: info.id.clone(),
                info: info.clone(),
                viewport_id: i as u32,
                files: Vec::new(),
            })
            .collect();
        let mut main = Vec::new();
        for (f, id) in category.iter().enumerate() {
            match id
                .as_ref()
                .and_then(|id| sections.iter_mut().find(|s| s.id == *id))
            {
                Some(section) => section.files.push(f as u32),
                None => main.push(f as u32),
            }
        }
        sections.retain(|s| !s.files.is_empty());
        Partition {
            categorizer,
            category,
            main,
            sections,
        }
    }

    fn section(&self, viewport_id: u32) -> Option<&SectionInfo> {
        self.sections.iter().find(|s| s.viewport_id == viewport_id)
    }
}

/// Emitted by a [`ReviewTab`] after every partition it applies (attach,
/// settings reload, palette toggle, [`DiffRefreshed`]).
pub struct Repartitioned;

impl EventEmitter<Repartitioned> for ReviewTab {}

/// A tab's category state (the [`categories`] extension).
pub struct Categories {
    /// Palette toggles: a built-in's on/off for this tab, over settings.json.
    overrides: BTreeMap<BuiltinCategory, bool>,
    /// The settings the last partition followed.
    settings: Option<Arc<Settings>>,
    /// Each category's open state, as last shown.
    open: BTreeMap<CategoryId, bool>,
    /// The user opened or closed a section (a band, an explicit target, a
    /// section action).
    touched: bool,
    /// A saved `open_sections` was applied.
    restored: bool,
    /// The waiting-question rule ran.
    questions_checked: bool,
    /// Background partitions started; only the latest one's result counts.
    generation: u64,
    task: Option<Task<()>>,
}

/// The current partition, as a tab extension.
struct Current(Arc<Partition>);

/// The shared categorizer of one settings load, compiled on first use.
struct Shared {
    settings: Arc<Settings>,
    categorizer: OnceCell<Arc<Categorizer>>,
}

impl Global for Shared {}

/// Registers the palette actions and follows settings loads (logging
/// unknown category keys and groups once per load).
pub fn init(cx: &mut App) {
    register_actions(cx);
    if let Some(store) = cx.try_global::<SettingsStore>() {
        let settings = store.shared();
        loaded(settings, cx);
    }
    cx.observe_global::<SettingsStore>(|cx| {
        let settings = SettingsStore::global(cx).shared();
        let same = cx
            .try_global::<Shared>()
            .is_some_and(|s| Arc::ptr_eq(&s.settings, &settings));
        if !same {
            loaded(settings, cx);
        }
    })
    .detach();
}

/// A new settings load: logs what `categories` holds that means nothing
/// and forgets the previous load's categorizer.
fn loaded(settings: Arc<Settings>, cx: &mut App) {
    for key in unknown_keys(&settings.categories) {
        tracing::warn!("settings.json: unknown key {key} (ignored)");
    }
    for group in unknown_groups(&settings.categories) {
        tracing::warn!("settings.json: unknown category group {group} (ignored)");
    }
    cx.set_global(Shared {
        settings,
        categorizer: OnceCell::new(),
    });
}

/// The categorizer of the settings in effect (`categories` and
/// `diff.generated_patterns`), compiled once per settings load and shared
/// by every tab. Its warnings (dropped legacy patterns, unknown icons) are
/// logged when it is compiled. Before the settings load, the defaults'.
pub fn shared(cx: &App) -> Arc<Categorizer> {
    let Some(store) = cx.try_global::<SettingsStore>() else {
        return Arc::new(compile(&Settings::default(), &BTreeMap::new()));
    };
    let settings = store.shared();
    match cx.try_global::<Shared>() {
        Some(s) if Arc::ptr_eq(&s.settings, &settings) => s
            .categorizer
            .get_or_init(|| {
                let categorizer = compile(&settings, &BTreeMap::new());
                for warning in categorizer.warnings() {
                    tracing::warn!("settings.json: {warning}");
                }
                Arc::new(categorizer)
            })
            .clone(),
        // A settings observer that runs before this module's.
        _ => Arc::new(compile(&settings, &BTreeMap::new())),
    }
}

/// The categorizer an open builds its provider with, captured on the main
/// thread before the open's background work: [`shared`], or, before the
/// settings load (the launch's open starts first), the settings file's,
/// read and compiled with the open.
pub(crate) enum ForOpen {
    Shared(Arc<Categorizer>),
    File(PathBuf),
}

impl ForOpen {
    pub(crate) fn capture(cx: &App) -> ForOpen {
        if cx.has_global::<SettingsStore>() {
            return ForOpen::Shared(shared(cx));
        }
        match cx.try_global::<crate::app_state::AppState>() {
            Some(state) => ForOpen::File(state.paths.config_dir.join(SETTINGS_FILE)),
            None => ForOpen::Shared(shared(cx)),
        }
    }

    /// The categorizer (off the main thread for [`ForOpen::File`]; an
    /// unreadable or invalid file counts as the defaults, as at startup).
    pub(crate) fn get(self) -> Arc<Categorizer> {
        match self {
            ForOpen::Shared(categorizer) => categorizer,
            ForOpen::File(path) => {
                let settings = crate::settings::loader::load(&path).unwrap_or_default();
                Arc::new(compile(&settings, &BTreeMap::new()))
            }
        }
    }
}

/// `settings`' categorizer with `overrides` (palette toggles) on top.
/// Settings in effect were validated when loaded and a toggle only flips
/// `enabled`, so this compiles; if it ever does not, the defaults apply.
fn compile(settings: &Settings, overrides: &BTreeMap<BuiltinCategory, bool>) -> Categorizer {
    let mut config = Cow::Borrowed(&settings.categories);
    for (&category, &on) in overrides {
        config.to_mut().get_mut(category).enabled = on;
    }
    Categorizer::new(&config, &settings.diff.generated_patterns).unwrap_or_else(|e| {
        tracing::warn!("categories: {e}; using the defaults");
        Categorizer::new(&CategoriesConfig::default(), &[])
            .unwrap_or_else(|e| unreachable!("the default categories compile: {e}"))
    })
}

/// The path a file is categorized by: its display path, or for a path that
/// is not UTF-8, its bytes read lossily.
fn path_of(file: &FileChange) -> Cow<'_, str> {
    match file.new_path.as_ref().or(file.old_path.as_ref()) {
        Some(p) if p.escaped => Cow::Owned(String::from_utf8_lossy(&p.to_bytes()).into_owned()),
        _ => Cow::Borrowed(file.display_path()),
    }
}

/// Whether `file` (as stored) is generated, by `categorizer`.
fn is_generated(categorizer: &Categorizer, file: &FileChange) -> bool {
    categorizer.is_generated(&path_of(file), file.generated_attr, file.generated)
}

/// The files of `stored` whose verdict differs from `shown`'s flag, with
/// their verdict.
fn verdict_changes(
    categorizer: &Categorizer,
    stored: &[FileChange],
    shown: &[FileChange],
) -> Vec<(u32, bool)> {
    stored
        .iter()
        .zip(shown)
        .enumerate()
        .filter_map(|(f, (stored, shown))| {
            let generated = is_generated(categorizer, stored);
            (generated != shown.generated).then_some((f as u32, generated))
        })
        .collect()
}

/// `files` with each `generated` flag set to its verdict: the same list
/// when no verdict differs, else a copy.
pub(crate) fn with_verdicts(
    files: &Arc<Vec<FileChange>>,
    categorizer: &Categorizer,
) -> Arc<Vec<FileChange>> {
    let changes = verdict_changes(categorizer, files, files);
    if changes.is_empty() {
        return files.clone();
    }
    let mut copy = (**files).clone();
    for (f, generated) in changes {
        copy[f as usize].generated = generated;
    }
    Arc::new(copy)
}

/// The tab's current partition.
pub fn partition(tab: &ReviewTab) -> Option<Arc<Partition>> {
    tab.extension::<Current>().map(|c| c.0.clone())
}

/// The tab's category state.
pub fn categories(tab: &ReviewTab) -> Option<Entity<Categories>> {
    tab.extension::<Entity<Categories>>().cloned()
}

/// Partitions a new tab before its first frame and follows its viewport's
/// sections, settings loads, refreshes and its threads' first load. Runs
/// after threads and before view state, which restores the open sections.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    let settings = cx.try_global::<SettingsStore>().map(SettingsStore::shared);
    let entity = cx.new(|_| Categories {
        overrides: BTreeMap::new(),
        settings,
        open: BTreeMap::new(),
        touched: false,
        restored: false,
        questions_checked: false,
        generation: 0,
        task: None,
    });
    tab.insert_extension(entity);
    let partition = Partition::new(shared(cx), &tab.opened.files);
    apply(tab, partition, &[], cx);

    cx.subscribe(
        &tab.viewport,
        |tab: &mut ReviewTab, _, event: &ViewportEvent, cx| match *event {
            ViewportEvent::SectionToggled { id, open } => toggled(tab, id, open, cx),
            ViewportEvent::SectionMarkViewed(id) => mark_section_viewed(tab, id, cx),
            _ => {}
        },
    )
    .detach();
    cx.subscribe_self(|tab: &mut ReviewTab, _: &DiffRefreshed, cx| refreshed(tab, cx))
        .detach();
    cx.observe_global::<SettingsStore>(settings_changed)
        .detach();
    if let Some(model) = crate::threads::threads(tab).cloned() {
        cx.subscribe(
            &model,
            |tab: &mut ReviewTab, _, _: &crate::threads::ThreadsEvent, cx| {
                waiting_questions(tab, cx)
            },
        )
        .detach();
    }
}

/// Shows `partition` in the tab: relabels the files of `generated`, sets
/// the sections (each category's open state as kept, else open when every
/// file is categorized), then emits [`Repartitioned`].
fn apply(
    tab: &mut ReviewTab,
    partition: Partition,
    generated: &[(u32, bool)],
    cx: &mut Context<ReviewTab>,
) {
    let Some(entity) = categories(tab) else {
        return;
    };
    let every = partition.main.is_empty();
    let sections: Vec<Section> = {
        let state = entity.read(cx);
        partition
            .sections
            .iter()
            .map(|s| Section {
                id: s.viewport_id,
                label: s.info.label(s.files.len()).into(),
                icon: Some(format!("icons/{}.svg", s.info.icon).into()),
                files: s.files.clone(),
                open: state.open.get(&s.id).copied().unwrap_or(every),
            })
            .collect()
    };
    entity.update(cx, |state, cx| {
        for (s, shown) in partition.sections.iter().zip(&sections) {
            state.open.insert(s.id.clone(), shown.open);
        }
        cx.notify();
    });
    tab.viewport.update(cx, |v, cx| {
        if !generated.is_empty() {
            v.set_generated(generated, cx);
        }
        v.set_sections(sections, cx);
    });
    tab.insert_extension(Current(Arc::new(partition)));
    cx.emit(Repartitioned);
}

/// A settings load: partitions again when the categories or the generated
/// patterns changed.
fn settings_changed(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(entity) = categories(tab) else {
        return;
    };
    let settings = SettingsStore::global(cx).shared();
    let same = entity.read(cx).settings.as_ref().is_some_and(|old| {
        Arc::ptr_eq(old, &settings)
            || (old.categories == settings.categories
                && old.diff.generated_patterns == settings.diff.generated_patterns)
    });
    if same {
        entity.update(cx, |state, _| state.settings = Some(settings));
        return;
    }
    repartition(tab, cx);
}

/// Partitions the tab again on the background executor, with the settings
/// in effect and its palette toggles; only the latest one is applied.
fn repartition(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(entity) = categories(tab) else {
        return;
    };
    let settings = cx
        .try_global::<SettingsStore>()
        .map(SettingsStore::shared)
        .unwrap_or_default();
    let (generation, overrides) = entity.update(cx, |state, _| {
        state.generation += 1;
        state.settings = Some(settings.clone());
        (state.generation, state.overrides.clone())
    });
    let base = overrides.is_empty().then(|| shared(cx));
    let files = tab.opened.files.clone();
    let shown = tab.viewport.read(cx).document().files().clone();
    let task = cx.spawn(async move |tab, cx| {
        let work_files = files.clone();
        let (partition, generated) = cx
            .background_spawn(async move {
                let categorizer = base.unwrap_or_else(|| Arc::new(compile(&settings, &overrides)));
                let generated = verdict_changes(&categorizer, &work_files, &shown);
                (Partition::new(categorizer, &work_files), generated)
            })
            .await;
        tab.update(cx, |tab, cx| {
            landed(tab, generation, &files, partition, &generated, cx)
        })
        .ok();
    });
    entity.update(cx, |state, _| state.task = Some(task));
}

/// A background partition is done: applied when it is the latest and the
/// tab still shows the files it was computed for (else it runs again).
fn landed(
    tab: &mut ReviewTab,
    generation: u64,
    files: &Arc<Vec<FileChange>>,
    partition: Partition,
    generated: &[(u32, bool)],
    cx: &mut Context<ReviewTab>,
) {
    let Some(entity) = categories(tab) else {
        return;
    };
    if entity.read(cx).generation != generation {
        return;
    }
    if !Arc::ptr_eq(files, &tab.opened.files) {
        repartition(tab, cx);
        return;
    }
    apply(tab, partition, generated, cx);
}

/// The tab shows another diff (a refresh, an iteration): its files are
/// partitioned now, with the tab's categorizer, keeping each category's
/// open state; the viewport's anchor policy handles an anchor that is
/// hidden now.
fn refreshed(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(current) = partition(tab) else {
        return;
    };
    let categorizer = current.categorizer.clone();
    let shown = tab.viewport.read(cx).document().files().clone();
    let generated = verdict_changes(&categorizer, &tab.opened.files, &shown);
    let partition = Partition::new(categorizer, &tab.opened.files);
    apply(tab, partition, &generated, cx);
}

/// The viewport opened or closed a section (its band, an explicit target,
/// Hide Section): kept for the tab and saved with the view state.
fn toggled(tab: &mut ReviewTab, viewport_id: u32, open: bool, cx: &mut Context<ReviewTab>) {
    let (Some(entity), Some(current)) = (categories(tab), partition(tab)) else {
        return;
    };
    let Some(section) = current.section(viewport_id) else {
        return;
    };
    // The viewport notified too, so the view state saves it.
    entity.update(cx, |state, cx| {
        state.open.insert(section.id.clone(), open);
        state.touched = true;
        cx.notify();
    });
}

/// The open sections to save with the view state: `None` until the user
/// opened or closed one or a saved state was applied (the default rule then
/// decides on reopening).
pub(crate) fn open_sections(tab: &ReviewTab, cx: &App) -> Option<Vec<String>> {
    let state = categories(tab)?.read(cx);
    (state.touched || state.restored).then(|| {
        state
            .open
            .iter()
            .filter(|(_, open)| **open)
            .map(|(id, _)| id.to_string())
            .collect()
    })
}

/// Applies a saved `open_sections` (the view-state restore calls it before
/// it restores the anchor): exactly those categories open. It wins over the
/// default and waiting-question rules.
pub fn apply_open_sections(tab: &mut ReviewTab, saved: &[String], cx: &mut Context<ReviewTab>) {
    let (Some(entity), Some(current)) = (categories(tab), partition(tab)) else {
        return;
    };
    let ids: Vec<CategoryId> = saved.iter().filter_map(|s| s.parse().ok()).collect();
    entity.update(cx, |state, _| {
        state.restored = true;
        for s in &current.sections {
            state.open.insert(s.id.clone(), false);
        }
        for id in &ids {
            state.open.insert(id.clone(), true);
        }
    });
    tab.viewport.update(cx, |v, cx| {
        for s in &current.sections {
            v.set_section_open(s.viewport_id, ids.contains(&s.id), cx);
        }
    });
}

/// The tab's threads changed; the first time they are loaded, sections
/// holding an agent question waiting on you open (OQ-53), unless a saved
/// state was applied or the user opened or closed a section.
fn waiting_questions(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let (Some(entity), Some(current)) = (categories(tab), partition(tab)) else {
        return;
    };
    if entity.read(cx).questions_checked {
        return;
    }
    let Some(files) = crate::threads::files_with_waiting_questions(tab, cx) else {
        return;
    };
    let skip = entity.update(cx, |state, _| {
        state.questions_checked = true;
        state.touched || state.restored
    });
    if skip {
        return;
    }
    let opening: Vec<&SectionInfo> = current
        .sections
        .iter()
        .filter(|s| s.files.iter().any(|f| files.contains(f)))
        .collect();
    entity.update(cx, |state, _| {
        for s in &opening {
            state.open.insert(s.id.clone(), true);
        }
    });
    tab.viewport.update(cx, |v, cx| {
        for s in &opening {
            v.set_section_open(s.viewport_id, true, cx);
        }
    });
}

/// A band's Mark all viewed (or the action): every file of the section
/// viewed, or unviewed once all are (design §9).
fn mark_section_viewed(tab: &mut ReviewTab, viewport_id: u32, cx: &mut Context<ReviewTab>) {
    let Some(current) = partition(tab) else {
        return;
    };
    let Some(section) = current.section(viewport_id) else {
        return;
    };
    let states = crate::viewed::states(tab).unwrap_or_default();
    let all_viewed = section
        .files
        .iter()
        .all(|&f| states.get(f as usize) == Some(&ViewedState::Viewed));
    crate::viewed::set_viewed(tab, &section.files, !all_viewed, cx);
    tab.viewport.update(cx, |v, cx| {
        v.set_band_all_viewed(viewport_id, !all_viewed, cx)
    });
}

/// The file the section actions act on: the cursor's, else the anchor's.
fn current_file(v: &DiffViewport) -> u32 {
    v.cursor().map_or(v.anchor().file_idx, |c| c.file_idx)
}

fn register_actions(cx: &mut App) {
    macro_rules! toggles {
        ($($action:ident => $category:ident),* $(,)?) => {$(
            handlers::on_action(cx, |tab: &mut ReviewTab, _: &actions::$action, _, cx| {
                toggle(tab, BuiltinCategory::$category, cx)
            });
        )*};
    }
    toggles![
        ToggleTests => Tests,
        ToggleGenerated => Generated,
        ToggleVendored => Vendored,
        ToggleAgents => Agents,
        ToggleDocs => Docs,
        ToggleTooling => Tooling,
        ToggleStories => Stories,
    ];
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::ShowNextSection, _, cx| show_next_section(tab, cx),
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::HideSection, _, cx| hide_section(tab, cx),
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::MarkSectionViewed, _, cx| mark_section(tab, cx),
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &actions::ExplainFile, window, cx| explain_file(tab, window, cx),
    );
}

/// Toggle Tests (and the other built-ins): the category's resulting on/off
/// is kept for this tab and wins over `settings.json` from now on.
fn toggle(tab: &mut ReviewTab, category: BuiltinCategory, cx: &mut Context<ReviewTab>) {
    let (Some(entity), Some(current)) = (categories(tab), partition(tab)) else {
        return;
    };
    let on = current
        .categorizer
        .enabled()
        .iter()
        .any(|info| info.id == CategoryId::Builtin(category));
    entity.update(cx, |state, _| {
        state.overrides.insert(category, !on);
    });
    repartition(tab, cx);
}

/// Show Next Section: the first closed section at or after the current
/// file in display order opens, with the cursor on its first line (an
/// explicit target).
fn show_next_section(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(current) = partition(tab) else {
        return;
    };
    let v = tab.viewport.read(cx);
    let from = v.display_rank(current_file(v));
    let next = current.sections.iter().find(|s| {
        v.section_open(s.viewport_id) == Some(false) && v.display_rank(s.files[0]) >= from
    });
    if let Some(first) = next.map(|s| s.files[0]) {
        tab.viewport.update(cx, |v, cx| v.go_to_file(first, cx));
    }
}

/// Hide Section: the current file's section closes.
fn hide_section(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let v = tab.viewport.read(cx);
    let Some(id) = v.section_of(current_file(v)) else {
        return;
    };
    if v.section_open(id) != Some(true) {
        return;
    }
    tab.viewport
        .update(cx, |v, cx| v.set_section_open(id, false, cx));
    toggled(tab, id, false, cx);
}

/// Mark Section Viewed: like the band of the current file's section, else
/// of the next section below it.
fn mark_section(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(current) = partition(tab) else {
        return;
    };
    let v = tab.viewport.read(cx);
    let file = current_file(v);
    let rank = v.display_rank(file);
    let id = v.section_of(file).or_else(|| {
        current
            .sections
            .iter()
            .find(|s| v.display_rank(s.files[0]) > rank)
            .map(|s| s.viewport_id)
    });
    if let Some(id) = id {
        mark_section_viewed(tab, id, cx);
    }
}

/// Explain file category: the current file's verdict, in a toast.
fn explain_file(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(current) = partition(tab) else {
        return;
    };
    let file = current_file(tab.viewport.read(cx));
    let Some(change) = tab.opened.files.get(file as usize) else {
        return;
    };
    let text = explain(&current.categorizer, change);
    if let Some((_, main)) = crate::window::main_window(cx) {
        main.update(cx, |m, cx| m.toast_info(text.into(), window, cx));
    }
}

/// A file's verdict in words: its category and the pattern and source that
/// decided, or the rescues that kept it out of a category.
fn explain(categorizer: &Categorizer, file: &FileChange) -> String {
    let path = file.display_path();
    match categorizer.explain(&path_of(file), file.generated_attr, file.generated) {
        Explain::Matched(verdict) => {
            let title = title(categorizer, &verdict.category);
            let pattern = &verdict.pattern;
            match verdict.source {
                Source::Attribute => format!("{path}: {title} (linguist-generated attribute)"),
                Source::BuiltIn { group } => {
                    format!("{path}: {title} (pattern \"{pattern}\", built-in group \"{group}\")")
                }
                Source::Extra => {
                    format!("{path}: {title} (pattern \"{pattern}\" in settings.json)")
                }
                Source::Custom => {
                    format!("{path}: {title} (pattern \"{pattern}\", custom category)")
                }
            }
        }
        Explain::Uncategorized { rescued_by } if rescued_by.is_empty() => {
            format!("{path}: no category")
        }
        Explain::Uncategorized { rescued_by } => {
            let rescues: Vec<String> = rescued_by
                .iter()
                .map(|(id, pattern)| format!("\"{pattern}\" skips {}", title(categorizer, id)))
                .collect();
            format!("{path}: no category ({})", rescues.join(", "))
        }
    }
}

/// A category's title (`Tests`, a custom category's name).
fn title(categorizer: &Categorizer, id: &CategoryId) -> String {
    match categorizer.enabled().iter().find(|info| info.id == *id) {
        Some(info) => info.title.clone(),
        None => match id {
            CategoryId::Builtin(b) => b.def().title.to_owned(),
            CategoryId::Custom(id) => id.clone(),
        },
    }
}

/// The tab's sections for `debug_state`: each one's category, file paths,
/// open state and band indicators.
pub(crate) fn debug_sections(tab: &ReviewTab, cx: &App) -> Value {
    let Some(current) = partition(tab) else {
        return json!([]);
    };
    let v = tab.viewport.read(cx);
    let files = &tab.opened.files;
    current
        .sections
        .iter()
        .map(|s| {
            let flags = v.band_flags(s.viewport_id);
            let paths: Vec<&str> = s
                .files
                .iter()
                .filter_map(|&f| files.get(f as usize))
                .map(FileChange::display_path)
                .collect();
            json!({
                "category": s.id.to_string(),
                "files": paths,
                "open": v.section_open(s.viewport_id) == Some(true),
                "open_threads": flags.open_threads,
                "agent": flags.agent,
                "changed_since_viewed": flags.changed_since_viewed,
            })
        })
        .collect()
}
