//! Threads in the diff and the threads panel (design §8.2, §8.4, §8.6,
//! §11.6 "Threads").
//!
//! - [`ReviewThreads`] (one per review tab, [`threads`]): the tab's threads
//!   (`Core::threads(ThreadScope::Review(..), Viewer::Human, ..)`, drafts
//!   included) and their carry-forward positions in the diff on screen
//!   (`Core::positions`), loaded on the background executor at attach and
//!   on [`reload`] (T3.10 after a draft, reply or resolve; T3.13 on store
//!   events).
//! - [`placement`]: positions → blocks. A line thread sits below its
//!   (last) line on its side (split: that side's column, with a spacer on
//!   the other; unified: full width), an outdated one below the nearest
//!   mapped line, file threads under the header; review threads and threads
//!   whose file or side is gone live in the panel only.
//! - [`block`]: the thread block: root and flat replies with author, agent
//!   and Draft badges, Question and Outdated badges (with the original
//!   snippet); resolved threads and agent notes collapse to a one-line chip.
//! - [`panel`]: open threads first, outdated ones always listed; a click
//!   jumps to the thread.
//!
//! Updates touch only what changed: a file whose thread anchors changed gets
//! `set_blocks` (the viewport re-lays out that file only), a thread whose
//! content changed gets `invalidate_block` (re-measured on the next frame).
//! "Hide agent notes" (`tab::ToggleAgentNotes`, the display options menu,
//! settings `agent_notes.hidden`) removes notes from the diff and the
//! panel; the toolbar's threads button ([`toolbar`]) then carries a muted
//! dot. `.` /
//! `,` (`viewport::NextOpenThread` / `PrevOpenThread`) move the cursor to
//! the next or previous open thread across files (outdated included,
//! resolved skipped), expanding a collapsed file and the hidden context
//! the thread's line is in.

pub mod block;
pub mod keys;
pub mod panel;
pub mod placement;
pub mod toolbar;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    MenuItem, ScrollHandle, Task, WeakEntity, Window, div,
};
use polygloss_core::git::RepoInfo;
use polygloss_core::ids::DiffId;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AuthorKind, OpenedDiff, Position, ThreadFilter, ThreadKind, ThreadScope, ThreadStatus,
    ThreadView, Viewer,
};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{FileChange, FileKind, Side};
use polygloss_viewport::{
    BlockAnchor, BlockId, BlockSpec, CursorPos, DiffViewport, FileFlags, FileState, ScrollTarget,
};

use crate::app_state::AppState;
use crate::keymap::actions::{tab as tab_actions, viewport as viewport_actions};
use crate::keymap::handlers;
use crate::live::DiffRefreshed;
use crate::review_tab::ReviewTab;
use crate::settings::SettingsStore;
use crate::window::MenuKind;
use placement::{Changes, DiffOrder, PlacedThread, ThreadPlace};

pub use keys::{current_thread, focus_panel};
pub use toolbar::{agent_notes_entry, open_counts, toolbar_button};

/// Registers `.` / `,`, "Hide agent notes", the threads panel's keys and
/// their menu items.
pub fn init(cx: &mut App) {
    keys::init(cx);
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport_actions::NextOpenThread, _, cx| {
            jump_to_open_thread(tab, Step::Next, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport_actions::PrevOpenThread, _, cx| {
            jump_to_open_thread(tab, Step::Prev, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::ToggleAgentNotes, _, cx| {
            if let Some(model) = threads(tab).cloned() {
                model.update(cx, |m, cx| {
                    let hide = !m.hide_agent_notes;
                    m.set_hide_agent_notes(hide, cx);
                });
            }
        },
    );
    crate::window::add_menu_items(
        MenuKind::Review,
        vec![
            MenuItem::action("Next Open Thread", viewport_actions::NextOpenThread),
            MenuItem::action("Previous Open Thread", viewport_actions::PrevOpenThread),
            MenuItem::separator(),
            MenuItem::action("Hide Agent Notes", tab_actions::ToggleAgentNotes),
        ],
        cx,
    );
}

/// The tab's [`ReviewThreads`].
pub fn threads(tab: &ReviewTab) -> Option<&Entity<ReviewThreads>> {
    tab.extension::<Entity<ReviewThreads>>()
}

/// Loads the tab's threads again (after a change to them: a draft, reply,
/// resolve or an agent's comment). Only files and blocks that changed are
/// touched.
pub fn reload(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    if let Some(model) = threads(tab).cloned() {
        model.update(cx, |m, cx| m.reload(cx));
    }
}

/// Sets threads up on a new review tab and starts loading them.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    let hide = cx
        .try_global::<SettingsStore>()
        .is_some_and(|s| s.settings().agent_notes.hidden);
    let opened = &tab.opened;
    let diff_options = tab.viewport.read(cx).options().diff;
    let model = cx.new(|cx| {
        // New diff options (hide whitespace) change the diffs: the changed
        // blocks cached for ordering are recomputed with them.
        cx.observe(&tab.viewport, |m: &mut ReviewThreads, viewport, cx| {
            let options = viewport.read(cx).options().diff;
            if options != m.diff_options {
                m.diff_options = options;
                m.reload(cx);
            }
        })
        .detach();
        let mut m = ReviewThreads {
            review_id: tab.review_id.clone(),
            diff_id: opened.diff_id.clone(),
            files: opened.files.clone(),
            file_index: placement::file_index(&opened.files),
            repo: opened.repo.clone(),
            scratch: opened.live.as_ref().map(|l| l.scratch_objects.clone()),
            viewport: tab.viewport.clone(),
            this: cx.weak_entity(),
            blobs: None,
            threads: Vec::new(),
            index: HashMap::new(),
            positions: HashMap::new(),
            places: HashMap::new(),
            digests: HashMap::new(),
            changes: HashMap::new(),
            diff_options,
            applied: BTreeMap::new(),
            expanded: HashSet::new(),
            hide_agent_notes: hide,
            setting_hidden: hide,
            loaded: false,
            loaded_diff: None,
            generation: 0,
            loading: None,
            stats: ThreadsStats::default(),
            nav: None,
            extra: BTreeMap::new(),
            composers: None,
            panel_focus: cx.focus_handle(),
            selected: None,
            panel_scroll: ScrollHandle::new(),
        };
        m.reload(cx);
        m
    });
    cx.subscribe(&model, |tab: &mut ReviewTab, _, event, cx| match event {
        ThreadsEvent::Changed => {
            push_flags(tab, cx);
            cx.notify();
        }
    })
    .detach();
    cx.observe(&model, |_, _, cx| cx.notify()).detach();
    // The tab shows another diff (a live refresh, another iteration,
    // "Changes since last review"): place the threads in it again.
    cx.subscribe_self(|tab: &mut ReviewTab, _: &DiffRefreshed, cx| {
        let Some(model) = threads(tab).cloned() else {
            return;
        };
        let opened = tab.opened.clone();
        model.update(cx, |m, cx| m.set_diff(&opened, cx));
    })
    .detach();
    // `agent_notes.hidden` changed in settings.json: follow it.
    let weak = model.downgrade();
    cx.observe_global::<SettingsStore>(move |_, cx| {
        let hidden = SettingsStore::global(cx).settings().agent_notes.hidden;
        if let Some(model) = weak.upgrade() {
            model.update(cx, |m, cx| {
                if m.setting_hidden != hidden {
                    m.setting_hidden = hidden;
                    m.set_hide_agent_notes(hidden, cx);
                }
            });
        }
    })
    .detach();
    tab.insert_extension(model);
}

/// The files holding an agent question waiting on the human (design
/// §8.4), in file order; `None` until the tab's threads have loaded.
pub fn files_with_waiting_questions(tab: &ReviewTab, cx: &App) -> Option<Vec<u32>> {
    let m = threads(tab)?.read(cx);
    if !m.is_loaded() {
        return None;
    }
    let mut files: Vec<u32> = m
        .threads
        .iter()
        .filter(|t| t.awaiting_you())
        .filter_map(|t| m.places.get(&t.id)?.file_idx())
        .collect();
    files.sort_unstable();
    files.dedup();
    Some(files)
}

/// The threads panel.
pub fn render_panel(
    tab: &ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    let model = threads(tab)?.clone();
    Some(panel::render(&model, window, cx))
}

/// What [`ReviewThreads`] reports to its tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadsEvent {
    /// Threads, positions or what shows changed (the tab pushes the file
    /// flags and repaints the panel).
    Changed,
}

/// What a reload touched, for tests (`thread_update_invalidates_one_file_only`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadsStats {
    /// `set_blocks` calls per file.
    pub set_blocks: BTreeMap<u32, u32>,
    /// `invalidate_block` calls, in order.
    pub invalidated: Vec<BlockId>,
    /// Loads applied.
    pub loads: u32,
}

/// The last `.`/`,` jump: the thread it went to and the cursor it left, so
/// the next jump continues from that thread while the cursor stays there
/// (file threads leave the cursor alone).
#[derive(Debug, Clone, PartialEq, Eq)]
struct NavMark {
    thread_id: String,
    cursor: Option<CursorPos>,
}

/// A review tab's threads, their places in the diff and the blocks shown.
pub struct ReviewThreads {
    review_id: String,
    diff_id: DiffId,
    files: Arc<Vec<FileChange>>,
    file_index: HashMap<String, u32>,
    repo: RepoInfo,
    /// The live snapshot's scratch object store (live tabs).
    scratch: Option<PathBuf>,
    viewport: Entity<DiffViewport>,
    this: WeakEntity<ReviewThreads>,
    blobs: Option<BlobReader>,
    threads: Vec<Arc<ThreadView>>,
    index: HashMap<String, usize>,
    positions: HashMap<String, Position>,
    places: HashMap<String, ThreadPlace>,
    /// A digest of what each thread's block shows.
    digests: HashMap<String, u64>,
    /// The changed blocks of files with old-side threads, computed at load
    /// with the diff options then in effect, to order threads in files the
    /// viewport has not loaded ([`ReviewThreads::changes`]; entries for
    /// other options are ignored, and recomputed on the next load).
    changes: HashMap<u32, (DiffOptions, Arc<Changes>)>,
    /// The viewport's diff options as last seen (a change reloads).
    diff_options: DiffOptions,
    /// The blocks the viewport has, per file.
    applied: BTreeMap<u32, Vec<(BlockId, BlockAnchor)>>,
    /// Collapsed threads (resolved, notes) the user opened.
    expanded: HashSet<String>,
    hide_agent_notes: bool,
    /// `agent_notes.hidden` as last applied from the settings.
    setting_hidden: bool,
    loaded: bool,
    /// The diff the last load listed the threads for.
    loaded_diff: Option<DiffId>,
    generation: u64,
    loading: Option<Task<()>>,
    stats: ThreadsStats,
    nav: Option<NavMark>,
    /// Blocks other features show in the diff with the threads (T3.10's
    /// composers), per file: after the threads at the same anchor.
    extra: BTreeMap<u32, Vec<BlockSpec>>,
    /// The tab's composers (T3.10), for the thread cards' footers.
    composers: Option<WeakEntity<crate::composer::Composers>>,
    /// The threads panel's keyboard focus (key context `ThreadsPanel`).
    panel_focus: FocusHandle,
    /// The panel's selected row (a thread id).
    selected: Option<String>,
    panel_scroll: ScrollHandle,
}

impl EventEmitter<ThreadsEvent> for ReviewThreads {}

type Loaded = (
    BlobReader,
    Vec<ThreadView>,
    HashMap<String, Position>,
    HashMap<u32, (DiffOptions, Arc<Changes>)>,
);

impl ReviewThreads {
    /// The first load has landed.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn review_id(&self) -> &str {
        &self.review_id
    }

    /// The threads panel's focus handle (key context `ThreadsPanel`).
    pub fn panel_focus(&self) -> &FocusHandle {
        &self.panel_focus
    }

    /// The thread selected in the panel.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Selects row `ix` of the panel (the last one past the end) and
    /// scrolls it into view.
    pub fn select_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = panel::rows(self, cx);
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let ix = ix.min(last);
        self.selected = Some(rows[ix].0.clone());
        // The list's children: the rows, with the "Resolved" title before
        // the first resolved one.
        let title = rows[..=ix].iter().any(|(_, open)| !open);
        self.panel_scroll.scroll_to_item(ix + usize::from(title));
        cx.notify();
    }

    /// The threads panel list's scroll position.
    pub(crate) fn panel_scroll(&self) -> &ScrollHandle {
        &self.panel_scroll
    }

    /// Every thread of the tab (drafts included), oldest first.
    pub fn threads(&self) -> impl Iterator<Item = &ThreadView> {
        self.threads.iter().map(|t| &**t)
    }

    pub fn thread(&self, id: &str) -> Option<&ThreadView> {
        self.index.get(id).map(|&i| &*self.threads[i])
    }

    pub(crate) fn thread_arc(&self, id: &str) -> Option<Arc<ThreadView>> {
        self.index.get(id).map(|&i| self.threads[i].clone())
    }

    /// The thread's carry-forward position in the diff on screen.
    pub fn position(&self, id: &str) -> Option<&Position> {
        self.positions.get(id)
    }

    /// Where the thread shows.
    pub fn place(&self, id: &str) -> Option<&ThreadPlace> {
        self.places.get(id)
    }

    pub fn files(&self) -> &[FileChange] {
        &self.files
    }

    pub fn hide_agent_notes(&self) -> bool {
        self.hide_agent_notes
    }

    /// Whether a thread that starts collapsed (resolved, agent note) was
    /// opened.
    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    pub fn stats(&self) -> &ThreadsStats {
        &self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats = ThreadsStats::default();
    }

    /// Whether `t` shows at all (hidden agent notes do not).
    pub fn shows(&self, t: &ThreadView) -> bool {
        !(self.hide_agent_notes && t.kind == ThreadKind::Note)
    }

    /// Whether `t` shows as a one-line chip: resolved threads and agent
    /// notes, until opened.
    pub fn collapsed(&self, t: &ThreadView) -> bool {
        (t.status == ThreadStatus::Resolved || t.kind == ThreadKind::Note)
            && !self.expanded.contains(&t.id)
    }

    /// Opens or closes a collapsed thread.
    pub fn toggle_expanded(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
        }
        // Only a block the viewport has (a hidden note or a panel-only
        // thread opens in the panel).
        let block = placement::block_id(id);
        if self.applied.values().flatten().any(|(b, _)| *b == block) {
            self.viewport
                .update(cx, |v, cx| v.invalidate_block(block, cx));
        }
        cx.notify();
    }

    /// Shows or hides agent notes in the diff and the panel.
    pub fn set_hide_agent_notes(&mut self, hide: bool, cx: &mut Context<Self>) {
        if self.hide_agent_notes == hide {
            return;
        }
        self.hide_agent_notes = hide;
        self.update_blocks(&HashMap::new(), cx);
        cx.emit(ThreadsEvent::Changed);
        cx.notify();
    }

    /// Sets the blocks other features show in the diff next to the threads
    /// (T3.10's line and file composers): `(file, block)`, placed after the
    /// file's thread blocks at the same anchor. Only files whose blocks
    /// change are laid out again.
    pub fn set_extra_blocks(&mut self, blocks: Vec<(u32, BlockSpec)>, cx: &mut Context<Self>) {
        let mut extra: BTreeMap<u32, Vec<BlockSpec>> = BTreeMap::new();
        for (f, spec) in blocks {
            if (f as usize) < self.files.len() {
                extra.entry(f).or_default().push(spec);
            }
        }
        self.extra = extra;
        self.update_blocks(&HashMap::new(), cx);
    }

    /// The tab's composers (T3.10), which draw the cards' reply boxes,
    /// Resolve buttons and edit fields.
    pub fn composers(&self) -> Option<Entity<crate::composer::Composers>> {
        self.composers.as_ref()?.upgrade()
    }

    pub fn set_composers(&mut self, composers: WeakEntity<crate::composer::Composers>) {
        self.composers = Some(composers);
    }

    /// Thread `id`'s block changed without its thread changing (a reply box
    /// opened in it): measured again on the next frame.
    pub fn invalidate_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        let block = placement::block_id(id);
        if self.applied.values().flatten().any(|(b, _)| *b == block) {
            self.viewport
                .update(cx, |v, cx| v.invalidate_block(block, cx));
        }
        cx.notify();
    }

    /// The tab shows `opened` now (a [`DiffRefreshed`]): positions are
    /// computed for its diff and every thread's block is placed again (the
    /// viewport dropped them all when its provider was swapped).
    pub fn set_diff(&mut self, opened: &OpenedDiff, cx: &mut Context<Self>) {
        self.diff_id = opened.diff_id.clone();
        self.files = opened.files.clone();
        self.file_index = placement::file_index(&opened.files);
        self.repo = opened.repo.clone();
        let scratch = opened.live.as_ref().map(|l| l.scratch_objects.clone());
        if scratch.is_some() || scratch != self.scratch {
            // A new live state's objects are new files in its scratch store.
            self.blobs = None;
        }
        self.scratch = scratch;
        // Everything keyed by file index belongs to the old list.
        self.positions.clear();
        self.places.clear();
        self.changes.clear();
        self.applied.clear();
        self.digests.clear();
        self.nav = None;
        self.reload(cx);
        cx.emit(ThreadsEvent::Changed);
        cx.notify();
    }

    /// The diff the threads are placed in.
    pub fn diff_id(&self) -> &DiffId {
        &self.diff_id
    }

    /// The diff the threads were last loaded for (right after a `set_diff`
    /// the list is still the previous diff's until the reload lands).
    pub fn loaded_diff(&self) -> Option<&DiffId> {
        self.loaded_diff.as_ref()
    }

    /// Loads the threads and positions again on the background executor.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let core = AppState::global(cx).core.clone();
        let review_id = self.review_id.clone();
        let diff_id = self.diff_id.clone();
        let files = self.files.clone();
        let repo = self.repo.clone();
        let scratch = self.scratch.clone();
        let blobs = self.blobs.clone();
        let file_index = self.file_index.clone();
        let diff_options = self.viewport.read(cx).options().diff;
        let have: HashSet<u32> = self
            .changes
            .iter()
            .filter(|(_, (o, _))| *o == diff_options)
            .map(|(f, _)| *f)
            .collect();
        let work = cx.background_spawn(async move {
            let blobs = match blobs {
                Some(b) => b,
                None => {
                    let b = BlobReader::open(&repo)?;
                    match scratch {
                        Some(s) => b.with_scratch(&s)?,
                        None => b,
                    }
                }
            };
            let threads = core.threads(
                ThreadScope::Review(review_id),
                Viewer::Human,
                &ThreadFilter::default(),
            )?;
            let ids: Vec<String> = threads.iter().map(|t| t.id.clone()).collect();
            let positions = core.positions(&diff_id, &files, &ids, &blobs)?;
            let changes = old_side_changes(
                &files,
                &file_index,
                &positions,
                &have,
                &blobs,
                &diff_options,
            );
            anyhow::Ok((blobs, threads, positions, changes))
        });
        self.loading = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                m.loading = None;
                match result {
                    Ok(loaded) => m.apply(loaded, cx),
                    Err(e) => tracing::warn!("loading the threads of {}: {e:#}", m.review_id),
                }
            })
            .ok();
        }));
    }

    /// Takes a load in: the places, then only the blocks that changed.
    fn apply(&mut self, (blobs, threads, positions, changes): Loaded, cx: &mut Context<Self>) {
        self.blobs = Some(blobs);
        self.changes.extend(changes);
        self.index = threads
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();
        self.places = threads
            .iter()
            .map(|t| {
                (
                    t.id.clone(),
                    placement::place(positions.get(&t.id), &self.file_index),
                )
            })
            .collect();
        self.threads = threads.into_iter().map(Arc::new).collect();
        self.positions = positions;
        let digests: HashMap<String, u64> = self
            .threads
            .iter()
            .map(|t| (t.id.clone(), digest(t, self.positions.get(&t.id))))
            .collect();
        let old = std::mem::replace(&mut self.digests, digests);
        let changed: HashMap<String, bool> = self
            .digests
            .iter()
            .map(|(id, d)| (id.clone(), old.get(id) != Some(d)))
            .collect();
        self.expanded.retain(|id| self.index.contains_key(id));
        self.loaded = true;
        // A load that lands is the latest one, started after the last
        // `set_diff`: its list is the one for the diff shown.
        self.loaded_diff = Some(self.diff_id.clone());
        self.stats.loads += 1;
        self.update_blocks(&changed, cx);
        cx.emit(ThreadsEvent::Changed);
        cx.notify();
    }

    /// Gives the viewport the blocks that changed: `set_blocks` for files
    /// whose anchors changed, `invalidate_block` for the other threads in
    /// `changed` whose content changed.
    fn update_blocks(&mut self, changed: &HashMap<String, bool>, cx: &mut Context<Self>) {
        let wanted = placement::by_file(
            self.threads
                .iter()
                // Right after `set_diff` the threads have no place in the new
                // diff until the reload lands.
                .filter(|t| self.shows(t))
                .filter_map(|t| Some((t.id.as_str(), *self.places.get(&t.id)?))),
        );
        let files: Vec<u32> = wanted
            .keys()
            .chain(self.applied.keys())
            .chain(self.extra.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut relaid: HashSet<u32> = HashSet::new();
        for f in files {
            let extra = self.extra.get(&f).map(Vec::as_slice).unwrap_or_default();
            let new: Vec<(BlockId, BlockAnchor)> = wanted
                .get(&f)
                .map(|v| v.iter().map(|p| (p.block, p.anchor)).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .chain(extra.iter().map(|b| (b.id, b.anchor)))
                .collect();
            let old = self.applied.get(&f).cloned().unwrap_or_default();
            if new == old {
                continue;
            }
            let specs: Vec<BlockSpec> = wanted
                .get(&f)
                .map(|v| v.iter().map(|p| self.spec(p)).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .chain(extra.iter().cloned())
                .collect();
            self.viewport.update(cx, |v, cx| v.set_blocks(f, specs, cx));
            *self.stats.set_blocks.entry(f).or_default() += 1;
            relaid.insert(f);
            if new.is_empty() {
                self.applied.remove(&f);
            } else {
                self.applied.insert(f, new);
            }
        }
        for placed in wanted.iter().filter(|(f, _)| !relaid.contains(f)) {
            for p in placed.1 {
                if changed.get(&p.thread_id).copied().unwrap_or(false) {
                    let block = p.block;
                    self.viewport
                        .update(cx, |v, cx| v.invalidate_block(block, cx));
                    self.stats.invalidated.push(block);
                }
            }
        }
    }

    /// The viewport block of a placed thread: rendered from this model at
    /// paint time.
    fn spec(&self, p: &PlacedThread) -> BlockSpec {
        let model = self.this.clone();
        let id = p.thread_id.clone();
        BlockSpec {
            id: p.block,
            anchor: p.anchor,
            render: std::rc::Rc::new(move |window, cx| match model.upgrade() {
                Some(model) => block::render(&model, &id, window, cx),
                None => div().into_any_element(),
            }),
        }
    }

    /// Per file: open threads (agent notes do not count; they wait on no
    /// one) and whether any thread is an agent's.
    pub fn file_counts(&self) -> Vec<(u32, bool)> {
        let mut out = vec![(0u32, false); self.files.len()];
        for t in &self.threads {
            let Some(f) = self.places.get(&t.id).and_then(ThreadPlace::file_idx) else {
                continue;
            };
            let entry = &mut out[f as usize];
            if t.status == ThreadStatus::Open && t.kind != ThreadKind::Note {
                entry.0 += 1;
            }
            if t.created_by.kind == AuthorKind::Agent {
                entry.1 = true;
            }
        }
        out
    }

    /// The open threads `.` / `,` visit, in display order: shown, placed in
    /// the diff, not resolved (outdated included), in closed sections too
    /// (going to one opens it).
    fn nav_targets(&self, order: &mut DiffOrderer<'_>) -> Vec<NavTarget> {
        let mut out: Vec<_> = self
            .threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.status == ThreadStatus::Open && self.shows(t))
            .filter_map(|(i, t)| {
                let place = self.places.get(&t.id).copied()?;
                (place != ThreadPlace::Panel)
                    .then(|| ((order.place(&place), i), t.id.clone(), place))
            })
            .collect();
        out.sort_by_key(|(key, _, _)| *key);
        out
    }

    /// File `f`'s changed blocks: from the viewport's loaded diff (its
    /// current options), else as computed at load for files with old-side
    /// threads, when with the same options; `None` when neither has it
    /// (then lines order by number, which is exact for new-side lines
    /// alone).
    fn changes(&self, viewport: &DiffViewport, f: u32) -> Option<Arc<Changes>> {
        let doc = viewport.document();
        if f < doc.len()
            && let FileState::Materialized(file) = doc.state(f)
        {
            return Some(placement::changes_of(&file.diff).into());
        }
        let options = viewport.options().diff;
        self.changes
            .get(&f)
            .filter(|(o, _)| *o == options)
            .map(|(_, c)| c.clone())
    }
}

/// Orders places and lines of one model top to bottom as the viewport
/// shows them ([`placement::diff_order`]: files by display rank, category
/// sections last), looking each file's changes up once.
pub(crate) struct DiffOrderer<'a> {
    model: &'a ReviewThreads,
    viewport: &'a DiffViewport,
    layout: polygloss_diff::rows::Layout,
    cache: HashMap<u32, Option<Arc<Changes>>>,
}

impl<'a> DiffOrderer<'a> {
    pub(crate) fn new(model: &'a ReviewThreads, viewport: &'a DiffViewport) -> Self {
        DiffOrderer {
            model,
            viewport,
            layout: viewport.effective_layout(),
            cache: HashMap::new(),
        }
    }

    fn file_changes(&mut self, f: u32) -> Option<Arc<Changes>> {
        let (model, viewport) = (self.model, self.viewport);
        self.cache
            .entry(f)
            .or_insert_with(|| model.changes(viewport, f))
            .clone()
    }

    pub(crate) fn place(&mut self, place: &ThreadPlace) -> DiffOrder {
        let changes = place.file_idx().and_then(|f| self.file_changes(f));
        let rank = place
            .file_idx()
            .map_or(u32::MAX, |f| self.viewport.display_rank(f));
        placement::diff_order(place, rank, changes.as_deref(), self.layout)
    }

    pub(crate) fn line(&mut self, file_idx: u32, side: Side, line: u32) -> DiffOrder {
        let changes = self.file_changes(file_idx);
        let rank = self.viewport.display_rank(file_idx);
        placement::line_diff_order(rank, side, line, changes.as_deref(), self.layout)
    }
}

/// The changed blocks of every file with an old-side line thread that
/// `have` lacks (a `.`/`,` or the panel may need to order its threads
/// before the viewport loads it). Files that cannot have both sides, or
/// whose blobs cannot be read, are skipped (their lines order by number).
fn old_side_changes(
    files: &[FileChange],
    file_index: &HashMap<String, u32>,
    positions: &HashMap<String, Position>,
    have: &HashSet<u32>,
    blobs: &BlobReader,
    options: &DiffOptions,
) -> HashMap<u32, (DiffOptions, Arc<Changes>)> {
    let wanted: std::collections::BTreeSet<u32> = positions
        .values()
        .filter_map(|p| match placement::place(Some(p), file_index) {
            ThreadPlace::Line {
                file_idx,
                side: Side::Old,
                ..
            } => Some(file_idx),
            _ => None,
        })
        .filter(|f| !have.contains(f))
        .collect();
    let mut out = HashMap::new();
    for f in wanted {
        let Some(file) = files.get(f as usize) else {
            continue;
        };
        if file.kind != FileKind::Text || file.old_path.is_none() || file.new_path.is_none() {
            continue;
        }
        let read = blobs
            .read(&file.old_blob)
            .and_then(|old| Ok((old, blobs.read(&file.new_blob)?)));
        match read {
            Ok((old, new)) => {
                let fd = polygloss_diff::hunks::diff_blobs(&old, &new, options);
                out.insert(f, (*options, placement::changes_of(&fd).into()));
            }
            Err(e) => tracing::debug!("reading {} to order its threads: {e}", file.display_path()),
        }
    }
    out
}

/// What a thread's block shows, hashed: a change means re-measuring it.
fn digest(t: &ThreadView, position: Option<&Position>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.kind.hash(&mut h);
    t.status.hash(&mut h);
    t.draft.hash(&mut h);
    t.resolved_by.hash(&mut h);
    t.anchor.hash(&mut h);
    position.hash(&mut h);
    for c in &t.comments {
        c.id.hash(&mut h);
        c.body_md.hash(&mut h);
        c.draft.hash(&mut h);
        c.deleted.hash(&mut h);
        c.edited_at.hash(&mut h);
    }
    h.finish()
}

/// Puts the threads' counts into the file flags of the viewport's headers
/// and the tree (keeping the Viewed state T3.7 put there).
fn push_flags(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads(tab) else {
        return;
    };
    let counts = model.read(cx).file_counts();
    let mut flags: Vec<FileFlags> = tab.viewport.read(cx).file_flags().to_vec();
    flags.resize(counts.len(), FileFlags::default());
    let mut changed = false;
    for (flag, (open, agent)) in flags.iter_mut().zip(counts) {
        if flag.open_threads != open || flag.agent_threads != agent {
            flag.open_threads = open;
            flag.agent_threads = agent;
            changed = true;
        }
    }
    if changed {
        tab.viewport
            .update(cx, |v, cx| v.set_file_flags(flags.clone(), cx));
    }
    // The tree too, even when the viewport already had them (the tree
    // ignores flags it already has).
    if let Some(tree) = crate::tree::file_tree(tab).cloned() {
        tree.update(cx, |t, cx| t.set_file_flags(flags, cx));
    }
}

/// Where a thread sits for `.` / `,`: its place in the diff, then its
/// index (threads on one line in creation order).
type NavKey = (DiffOrder, usize);

/// A `.` / `,` target: its key, thread id and place.
type NavTarget = (NavKey, String, ThreadPlace);

/// Which way `.` / `,` go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Next,
    Prev,
}

/// `.` / `,`: the cursor to the next or previous open thread across files,
/// wrapping around.
pub fn jump_to_open_thread(tab: &mut ReviewTab, step: Step, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads(tab).cloned() else {
        return;
    };
    let (targets, from, top) = {
        let m = model.read(cx);
        let v = tab.viewport.read(cx);
        let mut order = DiffOrderer::new(m, v);
        let targets = m.nav_targets(&mut order);
        let cursor = v.cursor();
        // From the last jump's thread while the cursor is where it left it,
        // else from the cursor.
        let marked = m
            .nav
            .as_ref()
            .filter(|mark| mark.cursor == cursor)
            .and_then(|mark| targets.iter().find(|t| t.1 == mark.thread_id))
            .map(|t| t.0);
        let from = marked.or_else(|| {
            cursor.map(|c| {
                let key = order.line(c.file_idx, c.side, c.line);
                (key, if step == Step::Next { usize::MAX } else { 0 })
            })
        });
        (targets, from, v.display_rank(v.anchor().file_idx))
    };
    if targets.is_empty() {
        return;
    }
    let pick = match (step, from) {
        (Step::Next, Some(k)) => targets.iter().find(|t| t.0 > k),
        (Step::Prev, Some(k)) => targets.iter().rev().find(|t| t.0 < k),
        (Step::Next, None) => targets.iter().find(|t| t.0.0.0 >= top),
        (Step::Prev, None) => targets.iter().rev().find(|t| t.0.0.0 < top),
    }
    .or(match step {
        Step::Next => targets.first(),
        Step::Prev => targets.last(),
    })
    .cloned();
    let Some((_, id, place)) = pick else {
        return;
    };
    let cursor = go_to(tab, &id, place, cx);
    model.update(cx, |m, cx| {
        m.selected = Some(id.clone());
        m.nav = Some(NavMark {
            thread_id: id,
            cursor,
        });
        cx.notify();
    });
}

/// Shows thread `id` at `place`: its file expanded, its line out of hidden
/// context, the cursor on it (a range for a range thread) or, for a file
/// thread, its block scrolled to. Returns the cursor it leaves.
pub fn go_to(
    tab: &mut ReviewTab,
    id: &str,
    place: ThreadPlace,
    cx: &mut Context<ReviewTab>,
) -> Option<CursorPos> {
    let Some(file_idx) = place.file_idx() else {
        return tab.viewport.read(cx).cursor();
    };
    let block = placement::block_id(id);
    tab.viewport.update(cx, |v, cx| {
        v.set_collapsed(file_idx, false, cx);
        match place {
            ThreadPlace::Line {
                side,
                start_line,
                line,
                ..
            } => {
                v.reveal_line(file_idx, side, line, cx);
                if start_line < line {
                    v.reveal_line(file_idx, side, start_line, cx);
                }
                v.set_cursor(
                    Some(CursorPos {
                        file_idx,
                        side,
                        line,
                        range_start: (start_line < line).then_some(start_line),
                    }),
                    cx,
                );
                if v.document().file_layout(file_idx).is_none() {
                    // Not laid out yet: lands once it is.
                    v.scroll_to(
                        ScrollTarget::Line {
                            file_idx,
                            side,
                            line,
                        },
                        cx,
                    );
                }
            }
            ThreadPlace::File { .. } => v.scroll_to(ScrollTarget::Block(block), cx),
            ThreadPlace::Panel => {}
        }
        v.cursor()
    })
}

/// Jumps to thread `id` from the panel: in the diff when it is placed
/// there (the viewport takes the keyboard), else it opens or closes in the
/// panel.
pub fn activate_thread(
    tab: &mut ReviewTab,
    id: &str,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(model) = threads(tab).cloned() else {
        return;
    };
    let (place, shows) = {
        let m = model.read(cx);
        let place = m.place(id).copied().unwrap_or(ThreadPlace::Panel);
        (place, m.thread(id).is_some_and(|t| m.shows(t)))
    };
    model.update(cx, |m, cx| {
        m.selected = Some(id.to_owned());
        cx.notify();
    });
    // Panel-only threads, and hidden agent notes (listed when outdated),
    // open under their row.
    if place == ThreadPlace::Panel || !shows {
        model.update(cx, |m, cx| m.toggle_expanded(id, cx));
        return;
    }
    // A collapsed thread opens where it lands.
    let collapsed = model
        .read(cx)
        .thread(id)
        .is_some_and(|t| model.read(cx).collapsed(t));
    if collapsed {
        model.update(cx, |m, cx| m.toggle_expanded(id, cx));
    }
    let cursor = go_to(tab, id, place, cx);
    model.update(cx, |m, _| {
        m.nav = Some(NavMark {
            thread_id: id.to_owned(),
            cursor,
        })
    });
    window.focus(&tab.viewport_focus().clone(), cx);
}
