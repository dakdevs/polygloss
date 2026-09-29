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
//! "Hide agent notes" (`tab::ToggleAgentNotes`, toolbar, settings
//! `agent_notes.hidden`) removes notes from the diff and the panel. `.` /
//! `,` (`viewport::NextOpenThread` / `PrevOpenThread`) move the cursor to
//! the next or previous open thread across files (outdated included,
//! resolved skipped), expanding a collapsed file and the hidden context
//! the thread's line is in.

pub mod block;
pub mod panel;
pub mod placement;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{IconName, Sizable as _};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, MenuItem, Task, WeakEntity, Window, div,
};
use polygloss_core::git::RepoInfo;
use polygloss_core::ids::DiffId;
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    AuthorKind, Position, ThreadFilter, ThreadKind, ThreadScope, ThreadStatus, ThreadView, Viewer,
};
use polygloss_diff::FileChange;
use polygloss_viewport::{
    BlockAnchor, BlockId, BlockSpec, CursorPos, DiffViewport, FileFlags, ScrollTarget,
};

use crate::app_state::AppState;
use crate::keymap::actions::{tab as tab_actions, viewport as viewport_actions};
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::settings::SettingsStore;
use crate::window::MenuKind;
use placement::{PlacedThread, ThreadPlace};

/// Registers `.` / `,`, "Hide agent notes" and their menu items.
pub fn init(cx: &mut App) {
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
    let model = cx.new(|cx| {
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
            applied: BTreeMap::new(),
            expanded: HashSet::new(),
            hide_agent_notes: hide,
            setting_hidden: hide,
            loaded: false,
            generation: 0,
            loading: None,
            stats: ThreadsStats::default(),
            nav: None,
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

/// "Hide agent notes" in the toolbar (design §11.4), when the review has
/// agent notes.
pub fn toolbar_items(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Vec<AnyElement> {
    let Some(model) = threads(tab) else {
        return Vec::new();
    };
    let m = model.read(cx);
    let notes = m
        .threads
        .iter()
        .filter(|t| t.kind == ThreadKind::Note)
        .count();
    if notes == 0 {
        return Vec::new();
    }
    let hidden = m.hide_agent_notes;
    let model = model.clone();
    vec![
        Button::new("toggle-agent-notes")
            .debug_selector(|| "toggle-agent-notes".into())
            .small()
            .ghost()
            .icon(if hidden {
                IconName::EyeOff
            } else {
                IconName::Eye
            })
            .label(if hidden {
                format!("Show agent notes ({notes})")
            } else {
                "Hide agent notes".to_owned()
            })
            .tooltip(if hidden {
                "Show the agents' notes in the diff"
            } else {
                "Hide the agents' notes in the diff"
            })
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| {
                    let hide = !m.hide_agent_notes;
                    m.set_hide_agent_notes(hide, cx);
                })
            })
            .into_any_element(),
    ]
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

/// The last `.`/`,` jump: where it went and the cursor it left, so the
/// next jump continues from that thread while the cursor stays there
/// (file threads leave the cursor alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NavMark {
    key: NavKey,
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
    /// The blocks the viewport has, per file.
    applied: BTreeMap<u32, Vec<(BlockId, BlockAnchor)>>,
    /// Collapsed threads (resolved, notes) the user opened.
    expanded: HashSet<String>,
    hide_agent_notes: bool,
    /// `agent_notes.hidden` as last applied from the settings.
    setting_hidden: bool,
    loaded: bool,
    generation: u64,
    loading: Option<Task<()>>,
    stats: ThreadsStats,
    nav: Option<NavMark>,
}

impl EventEmitter<ThreadsEvent> for ReviewThreads {}

type Loaded = (BlobReader, Vec<ThreadView>, HashMap<String, Position>);

impl ReviewThreads {
    /// The first load has landed.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn review_id(&self) -> &str {
        &self.review_id
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
        let block = placement::block_id(id);
        self.viewport
            .update(cx, |v, cx| v.invalidate_block(block, cx));
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
            anyhow::Ok((blobs, threads, positions))
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
    fn apply(&mut self, (blobs, threads, positions): Loaded, cx: &mut Context<Self>) {
        self.blobs = Some(blobs);
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
                .filter(|t| self.shows(t))
                .map(|t| (t.id.as_str(), self.places[&t.id])),
        );
        let files: Vec<u32> = wanted
            .keys()
            .chain(self.applied.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut relaid: HashSet<u32> = HashSet::new();
        for f in files {
            let new: Vec<(BlockId, BlockAnchor)> = wanted
                .get(&f)
                .map(|v| v.iter().map(|p| (p.block, p.anchor)).collect())
                .unwrap_or_default();
            let old = self.applied.get(&f).cloned().unwrap_or_default();
            if new == old {
                continue;
            }
            let specs: Vec<BlockSpec> = wanted
                .get(&f)
                .map(|v| v.iter().map(|p| self.spec(p)).collect())
                .unwrap_or_default();
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

    /// The open threads `.` / `,` visit, in diff order: shown, placed in the
    /// diff, not resolved (outdated included).
    fn nav_targets(&self) -> Vec<NavTarget> {
        let mut out: Vec<_> = self
            .threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.status == ThreadStatus::Open && self.shows(t))
            .filter_map(|(i, t)| {
                let place = self.places.get(&t.id).copied()?;
                (place != ThreadPlace::Panel).then(|| ((place.order(), i), t.id.clone(), place))
            })
            .collect();
        out.sort_by_key(|(key, _, _)| *key);
        out
    }
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
    if !changed {
        return;
    }
    tab.viewport
        .update(cx, |v, cx| v.set_file_flags(flags.clone(), cx));
    if let Some(tree) = crate::tree::file_tree(tab).cloned() {
        tree.update(cx, |t, cx| t.set_file_flags(flags, cx));
    }
}

/// Where a thread sits for `.` / `,`: its place's order, then its index
/// (threads on one line in creation order).
type NavKey = ((u32, u32, u8), usize);

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
    let (targets, mark) = {
        let m = model.read(cx);
        (m.nav_targets(), m.nav)
    };
    if targets.is_empty() {
        return;
    }
    let (cursor, top) = {
        let v = tab.viewport.read(cx);
        (v.cursor(), v.anchor().file_idx)
    };
    let from = match mark.filter(|m| m.cursor == cursor) {
        Some(mark) => Some(mark.key),
        None => cursor.map(|c| {
            let key = (c.file_idx, c.line + 1, placement::side_rank(c.side));
            (key, if step == Step::Next { usize::MAX } else { 0 })
        }),
    };
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
    let Some((key, id, place)) = pick else {
        return;
    };
    let cursor = go_to(tab, &id, place, cx);
    model.update(cx, |m, _| m.nav = Some(NavMark { key, cursor }));
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
    let (place, key) = {
        let m = model.read(cx);
        let place = m.place(id).copied().unwrap_or(ThreadPlace::Panel);
        let key = m.index.get(id).map(|&i| (place.order(), i));
        (place, key)
    };
    if place == ThreadPlace::Panel {
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
    if let Some(key) = key {
        model.update(cx, |m, _| m.nav = Some(NavMark { key, cursor }));
    }
    window.focus(&tab.viewport_focus().clone(), cx);
}
