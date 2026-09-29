//! The background materialization pipeline (design §12.4 "Window", §11.11,
//! §6.3 "Counts").
//!
//! Files near the viewport become [`MaterializedFile`]s off the main thread:
//! one load job per file (blobs → `diff_blobs` → word ranges of paired lines
//! → rows, see [`MaterializedFile::load`]) swapped in as a whole, then one
//! highlight job per side whose tokens swap in without moving anything.
//! Work is ordered by priority:
//!
//! 1. loads of the visible files, top to bottom, then their highlights;
//! 2. the rest of the materialization window, nearest to the viewport first
//!    (a file's load before its highlights);
//! 3. once a frame has painted every visible row ("first paint"), a pass over
//!    every file: blob sizes (height estimates), then added and removed line
//!    counts (header counts, better estimates), which also finds binary files
//!    that were listed as text.
//!
//! A fixed set of workers each pop the most urgent job whenever they are
//! free, so the order holds even while the queue changes under them; one
//! worker is always left to urgent work. A file that leaves the window (plus
//! [`CANCEL_SLACK_SCREENS`]) has its work cancelled: queued jobs are dropped,
//! running ones see their `AtomicUsize` flag at the next stage (lumis checks
//! it while parsing), and the document's per-file generation makes anything
//! that still arrives stale.
//!
//! Tokens are cached by `(blob, language, theme)` in a [`TokenCache`], so a
//! file that was evicted and comes back gets its tokens with its load. Files
//! with more than 100k lines on a side render without syntax until
//! [`crate::DiffViewport::highlight_anyway`] (design §11.11, OQ-14).

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use gpui_kit::{AppContext as _, Context};
use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, FileKind, GitPath, Oid, Side};
use polygloss_highlight::{
    Budget, HighlightError, Highlighter, Language, SyntaxTheme, ThemeId, TokenCache, Tokens,
    guess_language,
};

use crate::document::{Document, FileState, SizeHint};
use crate::materialize::{LoadError, LoadOptions, Loaded, MaterializedFile, is_binary, read_blob};
use crate::provider::DiffProvider;
use crate::view::{DiffViewport, ViewportOptions};

/// Bytes of tokens kept for files that are not materialized (**Provisional**).
pub const TOKEN_CACHE_BYTES: usize = 64 << 20;

/// Work for a file is cancelled only once it is this many screens beyond the
/// materialization window, so files at its edge do not flap between loading
/// and cancelled while the user scrolls back and forth.
pub const CANCEL_SLACK_SCREENS: f32 = 1.0;

/// "Highlight anyway" (design §11.11): no line limit and more time.
pub const HIGHLIGHT_ANYWAY_BUDGET: Budget = Budget {
    time: Duration::from_secs(10),
    max_lines: u32::MAX,
};

/// Files per background job: few main-thread hops, short enough that urgent
/// work never waits long for a worker (**Provisional**).
const SIZES_PER_JOB: u32 = 256;
const COUNTS_PER_JOB: u32 = 16;

/// Added and removed lines of one file (design §6.3 "Counts").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileCounts {
    pub additions: u32,
    pub deletions: u32,
}

/// Counters of the background pipeline since the viewport was created (tests
/// and the perf harness).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PipelineStats {
    /// Load jobs queued.
    pub loads: u64,
    /// Highlight jobs queued (one per side).
    pub highlights: u64,
    /// Loads and highlights cancelled because their file left the window or
    /// the options changed.
    pub cancelled: u64,
    /// Sides whose tokens came from the token cache instead of lumis.
    pub token_cache_hits: u64,
}

/// Whether a file's body comes from its blobs (text and symlinks whose
/// content changed); binary, generated, submodule and content-equal changes
/// are drawn from metadata alone.
pub(crate) fn needs_blobs(change: &FileChange) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink)
        && !change.generated
        && change.old_blob != change.new_blob
}

/// Whether the background pass counts a file's lines: text (generated
/// included) and symlinks whose content changed.
fn needs_counts(change: &FileChange) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink) && change.old_blob != change.new_blob
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Old => 0,
        Side::New => 1,
    }
}

const SIDES: [Side; 2] = [Side::Old, Side::New];

fn blob(change: &FileChange, side: Side) -> &Oid {
    match side {
        Side::Old => &change.old_blob,
        Side::New => &change.new_blob,
    }
}

/// The grammar for one side: its path, then its first bytes (a shebang).
fn side_language(path: Option<&GitPath>, text: &[u8]) -> Option<Language> {
    if text.is_empty() {
        return None;
    }
    guess_language(&path?.text, &text[..text.len().min(1024)])
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Workers: one core is left to the main thread (**Provisional**).
fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .saturating_sub(1)
        .clamp(1, 8)
}

/// A queued or running job, as the main thread tracks it.
#[derive(Debug, Clone)]
struct Flight {
    id: u64,
    cancel: Arc<AtomicUsize>,
}

impl Flight {
    fn cancel(&self) {
        self.cancel.store(1, Ordering::Relaxed);
    }
}

/// Where one side's tokens are.
#[derive(Debug, Clone, Default)]
enum Syntax {
    /// Tokens may still come: nothing is scheduled yet.
    #[default]
    Unknown,
    /// A highlight job is queued or running.
    Pending(Flight),
    /// Final for this version of the file: tokens swapped in, or none (no
    /// grammar, over the time budget, not UTF-8, a "Load diff" placeholder).
    Done,
    /// More than 100k lines: plain until "Highlight anyway".
    TooLarge,
}

/// The main thread's view of one file's background work.
#[derive(Debug, Default)]
struct FileWork {
    load: Option<Flight>,
    syntax: [Syntax; 2],
    languages: [Option<Language>; 2],
    counts: Option<FileCounts>,
    sizes: Option<(u64, u64)>,
    highlight_anyway: bool,
}

impl FileWork {
    fn busy(&self) -> bool {
        self.load.is_some() || self.syntax.iter().any(|s| matches!(s, Syntax::Pending(_)))
    }
}

/// A job for one file.
struct Job {
    flight: u64,
    file: u32,
    /// The document generation the job works for.
    generation: u64,
    cancel: Arc<AtomicUsize>,
    work: Work,
}

enum Work {
    Load {
        opts: LoadOptions,
        theme: ThemeId,
    },
    Highlight {
        side: Side,
        text: Arc<[u8]>,
        language: Language,
        budget: Budget,
        highlighter: Arc<Highlighter>,
    },
}

impl Job {
    fn highlight(&self) -> bool {
        matches!(self.work, Work::Highlight { .. })
    }
}

/// The pass over every file that starts after first paint.
struct Pass {
    len: u32,
    next_size: u32,
    next_count: u32,
    /// Counts of older epochs (other diff options) are dropped.
    epoch: u64,
    diff: DiffOptions,
}

enum Chunk {
    Sizes(Range<u32>),
    Counts {
        files: Range<u32>,
        epoch: u64,
        diff: DiffOptions,
    },
}

impl Pass {
    fn has_more(&self) -> bool {
        self.next_size < self.len || self.next_count < self.len
    }

    fn next(&mut self) -> Option<Chunk> {
        if self.next_size < self.len {
            let end = self.next_size.saturating_add(SIZES_PER_JOB).min(self.len);
            let files = self.next_size..end;
            self.next_size = end;
            return Some(Chunk::Sizes(files));
        }
        if self.next_count < self.len {
            let end = self.next_count.saturating_add(COUNTS_PER_JOB).min(self.len);
            let files = self.next_count..end;
            self.next_count = end;
            return Some(Chunk::Counts {
                files,
                epoch: self.epoch,
                diff: self.diff,
            });
        }
        None
    }
}

/// Work waiting for a worker. Urgent jobs are kept in priority order.
struct Queue {
    urgent: VecDeque<Job>,
    pass: Option<Pass>,
    running_background: usize,
    /// Background jobs never take the last worker.
    max_background: usize,
}

enum Popped {
    Job(Job),
    Chunk(Chunk),
}

impl Queue {
    fn background_ready(&self) -> bool {
        self.running_background < self.max_background
            && self.pass.as_ref().is_some_and(Pass::has_more)
    }

    fn has_work(&self) -> bool {
        !self.urgent.is_empty() || self.background_ready()
    }

    fn pop(&mut self) -> Option<Popped> {
        if let Some(job) = self.urgent.pop_front() {
            return Some(Popped::Job(job));
        }
        if !self.background_ready() {
            return None;
        }
        let chunk = self.pass.as_mut()?.next()?;
        self.running_background += 1;
        Some(Popped::Chunk(chunk))
    }
}

/// What the workers share with the main thread.
struct Shared {
    provider: Arc<dyn DiffProvider>,
    /// The files as the provider listed them (the workers read blob ids and
    /// paths; kinds found later live in the document).
    changes: Arc<Vec<FileChange>>,
    queue: Mutex<Queue>,
    tokens: Mutex<TokenCache>,
    /// Files whose counts are known, or that need none: the counts pass skips
    /// them. (A file whose load is running when the pass reaches it is read
    /// by both; skipping it would lose its counts if the load is cancelled.)
    counted: Box<[AtomicBool]>,
}

/// One file's line counts, or binary content.
pub(crate) enum Counted {
    Lines { counts: FileCounts, hunks: u32 },
    Binary,
}

/// A finished job, handed to the main thread.
pub(crate) enum Done {
    Loaded {
        flight: u64,
        file: u32,
        generation: u64,
        outcome: Outcome,
    },
    Tokens {
        flight: u64,
        file: u32,
        generation: u64,
        side: Side,
        theme: ThemeId,
        tokens: Option<Arc<Tokens>>,
        cached: bool,
    },
    Cancelled {
        flight: u64,
        file: u32,
    },
    Sizes(Vec<(u32, u64, u64)>),
    Counts {
        epoch: u64,
        files: Vec<(u32, Counted)>,
    },
}

pub(crate) enum Outcome {
    File {
        file: MaterializedFile,
        languages: [Option<Language>; 2],
        /// Sides whose tokens came from the cache.
        cached: u32,
        /// More changed lines than the "Load diff" threshold: no rows.
        large: bool,
    },
    Binary,
    Failed(String),
}

impl Shared {
    /// Pops and runs the most urgent job (on a worker). `None` when there is
    /// nothing a worker may take.
    fn run_next(&self) -> Option<Done> {
        let popped = lock(&self.queue).pop()?;
        Some(match popped {
            Popped::Job(job) => self.run_job(job),
            Popped::Chunk(chunk) => {
                let done = self.run_chunk(chunk);
                lock(&self.queue).running_background -= 1;
                done
            }
        })
    }

    fn run_job(&self, job: Job) -> Done {
        let Job {
            flight,
            file,
            generation,
            cancel,
            work,
        } = job;
        if cancel.load(Ordering::Relaxed) != 0 {
            return Done::Cancelled { flight, file };
        }
        let change = &self.changes[file as usize];
        match work {
            Work::Load { opts, theme } => {
                let outcome = match MaterializedFile::load(&*self.provider, change, &opts, &cancel)
                {
                    Err(LoadError::Cancelled) => return Done::Cancelled { flight, file },
                    Err(LoadError::Failed(e)) => Outcome::Failed(format!("{e:#}")),
                    Ok(Loaded::Binary) => Outcome::Binary,
                    Ok(Loaded::File(loaded)) => {
                        let large = loaded.is_large(opts.large_file_changed_lines);
                        self.with_cached_tokens(change, loaded, theme, large)
                    }
                };
                Done::Loaded {
                    flight,
                    file,
                    generation,
                    outcome,
                }
            }
            Work::Highlight {
                side,
                text,
                language,
                budget,
                highlighter,
            } => {
                let theme = highlighter.theme().id();
                let oid = blob(change, side).as_str();
                let done = |tokens: Option<Arc<Tokens>>, cached: bool| Done::Tokens {
                    flight,
                    file,
                    generation,
                    side,
                    theme,
                    tokens,
                    cached,
                };
                if let Some(tokens) = lock(&self.tokens).get(oid, language, theme) {
                    return done(Some(tokens), true);
                }
                match highlighter.highlight(&text, &language, &cancel, budget) {
                    Ok(tokens) => {
                        let tokens = Arc::new(tokens);
                        lock(&self.tokens).insert(oid, language, theme, tokens.clone());
                        done(Some(tokens), false)
                    }
                    Err(HighlightError::Cancelled) => Done::Cancelled { flight, file },
                    Err(HighlightError::BudgetExceeded | HighlightError::Unsupported) => {
                        done(None, false)
                    }
                }
            }
        }
    }

    /// The loaded file with both sides' languages, and the tokens the cache
    /// already has for them.
    fn with_cached_tokens(
        &self,
        change: &FileChange,
        file: MaterializedFile,
        theme: ThemeId,
        large: bool,
    ) -> Outcome {
        // A symlink's text is its target path: no grammar applies.
        let languages = if change.kind == FileKind::Symlink {
            [None, None]
        } else {
            [
                side_language(change.old_path.as_ref(), &file.old_text),
                side_language(change.new_path.as_ref(), &file.new_text),
            ]
        };
        let [old, new] = if large {
            [None, None]
        } else {
            let mut cache = lock(&self.tokens);
            SIDES.map(|side| {
                let language = languages[side_index(side)]?;
                cache.get(blob(change, side).as_str(), language, theme)
            })
        };
        let cached = u32::from(old.is_some()) + u32::from(new.is_some());
        let file = if cached > 0 {
            file.with_tokens(old, new)
        } else {
            file
        };
        Outcome::File {
            file,
            languages,
            cached,
            large,
        }
    }

    fn run_chunk(&self, chunk: Chunk) -> Done {
        match chunk {
            Chunk::Sizes(files) => {
                let size = |oid: &Oid| {
                    if oid.is_zero() {
                        Some(0)
                    } else {
                        self.provider.blob_size(oid).ok()
                    }
                };
                let sizes = files
                    .filter_map(|f| {
                        let change = &self.changes[f as usize];
                        if change.kind == FileKind::Submodule {
                            return None;
                        }
                        Some((f, size(&change.old_blob)?, size(&change.new_blob)?))
                    })
                    .collect();
                Done::Sizes(sizes)
            }
            Chunk::Counts { files, epoch, diff } => {
                let counted = files
                    .filter(|&f| !self.counted[f as usize].load(Ordering::Relaxed))
                    .filter_map(|f| Some((f, self.count(&self.changes[f as usize], &diff)?)))
                    .collect();
                Done::Counts {
                    epoch,
                    files: counted,
                }
            }
        }
    }

    /// Reads a file's blobs and counts its changed lines (nothing is kept).
    /// `None` when a blob cannot be read.
    fn count(&self, change: &FileChange, diff: &DiffOptions) -> Option<Counted> {
        let never = AtomicUsize::new(0);
        let sniff = change.kind == FileKind::Text;
        let old = read_blob(&*self.provider, &change.old_blob, &never).ok()?;
        if sniff && is_binary(&old) {
            return Some(Counted::Binary);
        }
        let new = read_blob(&*self.provider, &change.new_blob, &never).ok()?;
        if sniff && is_binary(&new) {
            return Some(Counted::Binary);
        }
        let d = diff_blobs(&old, &new, diff);
        Some(Counted::Lines {
            counts: FileCounts {
                additions: d.additions,
                deletions: d.deletions,
            },
            hunks: d.hunks.len() as u32,
        })
    }
}

/// What applying a result asks of the view.
#[derive(Debug, Default)]
pub(crate) struct Applied {
    /// Files whose rows or placeholder must be laid out again.
    pub relayout: Vec<u32>,
    /// Files found to be binary.
    pub binary: Vec<u32>,
    /// Resident bytes grew: check the eviction budget.
    pub grew: bool,
    /// Something the window shows changed: paint again. (Sizes and counts of
    /// files outside it only refine estimates, which never move what is on
    /// screen.)
    pub repaint: bool,
}

/// The main-thread side of the pipeline, owned by the view.
pub(crate) struct Pipeline {
    shared: Arc<Shared>,
    work: Vec<FileWork>,
    /// Files with a queued or running load or highlight.
    active: Vec<u32>,
    workers: usize,
    max_workers: usize,
    next_flight: u64,
    highlighter: Arc<Highlighter>,
    epoch: u64,
    background_started: bool,
    /// Schedule even if the window did not move (options changed).
    dirty: bool,
    last_window: Range<u32>,
    last_visible: Range<u32>,
    stats: PipelineStats,
}

impl Pipeline {
    pub fn new(
        provider: Arc<dyn DiffProvider>,
        changes: Arc<Vec<FileChange>>,
        theme: Arc<SyntaxTheme>,
    ) -> Pipeline {
        let mut work: Vec<FileWork> = changes.iter().map(|_| FileWork::default()).collect();
        for (w, change) in work.iter_mut().zip(changes.iter()) {
            // Content-equal changes (mode only, pure rename) change no line.
            if matches!(change.kind, FileKind::Text | FileKind::Symlink)
                && change.old_blob == change.new_blob
            {
                w.counts = Some(FileCounts {
                    additions: 0,
                    deletions: 0,
                });
            }
        }
        let counted = changes
            .iter()
            .map(|c| AtomicBool::new(!needs_counts(c)))
            .collect();
        let max_workers = worker_count();
        Pipeline {
            shared: Arc::new(Shared {
                provider,
                changes,
                queue: Mutex::new(Queue {
                    urgent: VecDeque::new(),
                    pass: None,
                    running_background: 0,
                    max_background: max_workers.saturating_sub(1).max(1),
                }),
                tokens: Mutex::new(TokenCache::new(TOKEN_CACHE_BYTES)),
                counted,
            }),
            work,
            active: Vec::new(),
            workers: 0,
            max_workers,
            next_flight: 0,
            highlighter: Arc::new(Highlighter::new(theme)),
            epoch: 0,
            background_started: false,
            dirty: true,
            last_window: 0..0,
            last_visible: 0..0,
            stats: PipelineStats::default(),
        }
    }

    pub fn stats(&self) -> PipelineStats {
        self.stats
    }

    pub fn counts(&self, f: u32) -> Option<FileCounts> {
        self.work.get(f as usize)?.counts
    }

    pub fn blob_sizes(&self, f: u32) -> Option<(u64, u64)> {
        self.work.get(f as usize)?.sizes
    }

    /// Whether `side` of file `f` may still get tokens (plain rows showing it
    /// count as unhighlighted, plan T2.9 `highlight_ms`).
    pub fn tokens_pending(&self, f: u32, side: Side) -> bool {
        self.work.get(f as usize).is_some_and(|w| {
            matches!(
                w.syntax[side_index(side)],
                Syntax::Unknown | Syntax::Pending(_)
            )
        })
    }

    /// Whether file `f` renders without syntax because it is too large.
    pub fn syntax_skipped(&self, f: u32) -> bool {
        self.work
            .get(f as usize)
            .is_some_and(|w| w.syntax.iter().any(|s| matches!(s, Syntax::TooLarge)))
    }

    /// "Highlight anyway" for file `f` (sticks across evictions and reloads).
    pub fn highlight_anyway(&mut self, f: u32) {
        let Some(w) = self.work.get_mut(f as usize) else {
            return;
        };
        w.highlight_anyway = true;
        for s in &mut w.syntax {
            if matches!(s, Syntax::TooLarge) {
                *s = Syntax::Unknown;
            }
        }
        self.dirty = true;
    }

    pub fn background_started(&self) -> bool {
        self.background_started
    }

    fn flight(&mut self) -> Flight {
        self.next_flight += 1;
        Flight {
            id: self.next_flight,
            cancel: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn mark_active(&mut self, f: u32) {
        if !self.active.contains(&f) {
            self.active.push(f);
        }
    }

    /// Queues work for the files in the window, cancels work for files that
    /// left it, and wakes workers. Cheap when nothing changed: a few checks
    /// per window file, no locking.
    pub fn schedule(
        &mut self,
        doc: &mut Document,
        files: &[FileChange],
        opts: &ViewportOptions,
        layout: Layout,
        cx: &mut Context<DiffViewport>,
    ) {
        if doc.is_empty() {
            return;
        }
        let h = doc.viewport_height();
        let visible = doc.visible(h);
        let window = doc.materialize_range(h, opts.window_screens);
        let keep = doc.materialize_range(h, opts.window_screens + CANCEL_SLACK_SCREENS);
        let mut changed = std::mem::take(&mut self.dirty)
            || window != self.last_window
            || visible != self.last_visible;

        let mut i = 0;
        while i < self.active.len() {
            let f = self.active[i];
            if keep.contains(&f) && !doc.is_collapsed(f) {
                i += 1;
                continue;
            }
            self.active.swap_remove(i);
            self.cancel_file(f, doc);
            changed = true;
        }

        let mut jobs = Vec::new();
        for f in window.clone() {
            if !doc.is_collapsed(f) {
                self.plan(f, &files[f as usize], doc, opts, layout, &mut jobs);
            }
        }
        if jobs.is_empty() && !changed {
            return;
        }
        self.last_window = window;
        self.last_visible = visible.clone();
        {
            let mut q = lock(&self.shared.queue);
            q.urgent.retain(|j| j.cancel.load(Ordering::Relaxed) == 0);
            q.urgent.extend(jobs);
            let (top, bottom) = (doc.scroll_top(), doc.scroll_top() + f64::from(h));
            q.urgent
                .make_contiguous()
                .sort_by_key(|j| priority(j.file, j.highlight(), doc, &visible, top, bottom));
        }
        self.wake(cx);
    }

    /// Queues what file `f` (in the window, expanded) still needs.
    fn plan(
        &mut self,
        f: u32,
        change: &FileChange,
        doc: &mut Document,
        opts: &ViewportOptions,
        layout: Layout,
        jobs: &mut Vec<Job>,
    ) {
        let idx = f as usize;
        let wants_load = needs_blobs(change)
            && self.work[idx].load.is_none()
            && matches!(
                doc.state(f),
                FileState::Estimated | FileState::Evicted | FileState::Loading { .. }
            );
        if wants_load {
            let generation = doc.begin_loading(f);
            let flight = self.flight();
            jobs.push(Job {
                flight: flight.id,
                file: f,
                generation,
                cancel: flight.cancel.clone(),
                work: Work::Load {
                    opts: LoadOptions {
                        diff: opts.diff,
                        word_diff: opts.word_diff,
                        large_file_changed_lines: opts.large_file_changed_lines,
                        rows: Some(layout),
                    },
                    theme: self.highlighter.theme().id(),
                },
            });
            self.work[idx].load = Some(flight);
            self.stats.loads += 1;
            self.mark_active(f);
            return;
        }
        if !opts.syntax {
            return;
        }
        let FileState::Materialized(file) = doc.state(f) else {
            return;
        };
        let generation = doc.generation(f);
        let lines = file.diff.old.len().max(file.diff.new.len());
        let too_large = lines > Budget::DEFAULT_MAX_LINES && !self.work[idx].highlight_anyway;
        for side in SIDES {
            let s = side_index(side);
            if !matches!(self.work[idx].syntax[s], Syntax::Unknown) {
                continue;
            }
            if too_large {
                self.work[idx].syntax[s] = Syntax::TooLarge;
                continue;
            }
            let text = match side {
                Side::Old => &file.old_text,
                Side::New => &file.new_text,
            };
            let language = self.work[idx].languages[s];
            let (Some(language), false) = (language, file.tokens(side).is_some()) else {
                self.work[idx].syntax[s] = Syntax::Done;
                continue;
            };
            let budget = if self.work[idx].highlight_anyway {
                HIGHLIGHT_ANYWAY_BUDGET
            } else {
                Budget::default()
            };
            let text = text.clone();
            let flight = self.flight();
            jobs.push(Job {
                flight: flight.id,
                file: f,
                generation,
                cancel: flight.cancel.clone(),
                work: Work::Highlight {
                    side,
                    text,
                    language,
                    budget,
                    highlighter: self.highlighter.clone(),
                },
            });
            self.work[idx].syntax[s] = Syntax::Pending(flight);
            self.stats.highlights += 1;
            self.mark_active(f);
        }
    }

    /// Cancels file `f`'s queued and running work.
    fn cancel_file(&mut self, f: u32, doc: &mut Document) {
        let w = &mut self.work[f as usize];
        if let Some(flight) = w.load.take() {
            flight.cancel();
            doc.cancel_loading(f);
            self.stats.cancelled += 1;
        }
        for s in &mut w.syntax {
            if let Syntax::Pending(flight) = s {
                flight.cancel();
                *s = Syntax::Unknown;
                self.stats.cancelled += 1;
            }
        }
    }

    /// Cancels everything queued or running.
    fn cancel_all(&mut self, doc: &mut Document) {
        for f in std::mem::take(&mut self.active) {
            self.cancel_file(f, doc);
        }
        lock(&self.shared.queue).urgent.clear();
    }

    /// Starts workers for the queued work (up to the worker count).
    fn wake(&mut self, cx: &mut Context<DiffViewport>) {
        let mut wanted = {
            let q = lock(&self.shared.queue);
            let background = if q.background_ready() {
                q.max_background - q.running_background
            } else {
                0
            };
            q.urgent.len() + background
        };
        while self.workers < self.max_workers && wanted > 0 {
            self.workers += 1;
            wanted -= 1;
            let shared = self.shared.clone();
            cx.spawn(async move |this, cx| {
                loop {
                    let worker = shared.clone();
                    let done = cx.background_spawn(async move { worker.run_next() }).await;
                    let more = this
                        .update(cx, |view, cx| view.pipeline_done(done, cx))
                        .unwrap_or(false);
                    if !more {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// After a worker's pop: whether it keeps going. A worker that found
    /// nothing stops, unless work arrived since.
    pub fn worker_continues(&mut self, had_job: bool) -> bool {
        if had_job || lock(&self.shared.queue).has_work() {
            return true;
        }
        self.workers -= 1;
        false
    }

    /// Starts the pass over every file (after first paint).
    pub fn start_background(&mut self, diff: DiffOptions, cx: &mut Context<DiffViewport>) {
        self.background_started = true;
        lock(&self.shared.queue).pass = Some(Pass {
            len: self.work.len() as u32,
            next_size: 0,
            next_count: 0,
            epoch: self.epoch,
            diff,
        });
        self.wake(cx);
    }

    /// Takes a finished job's result into the document. `window` is the
    /// materialization window.
    pub fn apply(&mut self, done: Done, doc: &mut Document, window: Range<u32>) -> Applied {
        let mut out = Applied::default();
        match done {
            Done::Cancelled { flight, file } => {
                let w = &mut self.work[file as usize];
                if w.load.as_ref().is_some_and(|l| l.id == flight) {
                    w.load = None;
                    doc.cancel_loading(file);
                }
                for s in &mut w.syntax {
                    if matches!(s, Syntax::Pending(p) if p.id == flight) {
                        *s = Syntax::Unknown;
                    }
                }
                self.settle(file);
            }
            Done::Loaded {
                flight,
                file,
                generation,
                outcome,
            } => {
                let w = &mut self.work[file as usize];
                if !w.load.as_ref().is_some_and(|l| l.id == flight) {
                    return out;
                }
                w.load = None;
                self.settle(file);
                self.apply_load(file, generation, outcome, doc, &mut out);
            }
            Done::Tokens {
                flight,
                file,
                generation,
                side,
                theme,
                tokens,
                cached,
            } => {
                let s = side_index(side);
                let w = &mut self.work[file as usize];
                if !matches!(&w.syntax[s], Syntax::Pending(p) if p.id == flight) {
                    return out;
                }
                w.syntax[s] = Syntax::Done;
                self.settle(file);
                let current = match doc.state(file) {
                    FileState::Materialized(current) if doc.generation(file) == generation => {
                        current.clone()
                    }
                    _ => {
                        self.work[file as usize].syntax[s] = Syntax::Unknown;
                        return out;
                    }
                };
                if theme != self.highlighter.theme().id() {
                    self.work[file as usize].syntax[s] = Syntax::Unknown;
                    return out;
                }
                if cached {
                    self.stats.token_cache_hits += 1;
                }
                // Even without tokens: its rows no longer wait for any.
                out.repaint = true;
                if tokens.is_some() {
                    let next = match side {
                        Side::Old => current.with_tokens(tokens, current.new_tokens.clone()),
                        Side::New => current.with_tokens(current.old_tokens.clone(), tokens),
                    };
                    doc.set_materialized(file, generation, Arc::new(next));
                    out.grew = true;
                }
            }
            Done::Sizes(sizes) => {
                for (f, old, new) in sizes {
                    self.work[f as usize].sizes = Some((old, new));
                    doc.set_size_hint(f, SizeHint::BlobSizes { old, new });
                    out.repaint |= window.contains(&f);
                }
            }
            Done::Counts { epoch, files } => {
                if epoch != self.epoch {
                    return out;
                }
                for (f, counted) in files {
                    self.shared.counted[f as usize].store(true, Ordering::Relaxed);
                    out.repaint |= window.contains(&f);
                    match counted {
                        Counted::Binary => out.binary.push(f),
                        Counted::Lines { counts, hunks } => {
                            let w = &mut self.work[f as usize];
                            if w.counts.is_none() {
                                w.counts = Some(counts);
                                doc.set_size_hint(f, counts_hint(counts, hunks));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn apply_load(
        &mut self,
        f: u32,
        generation: u64,
        outcome: Outcome,
        doc: &mut Document,
        out: &mut Applied,
    ) {
        match outcome {
            Outcome::File {
                file,
                languages,
                cached,
                large,
            } => {
                let counts = FileCounts {
                    additions: file.diff.additions,
                    deletions: file.diff.deletions,
                };
                let hunks = file.diff.hunks.len() as u32;
                let file = Arc::new(file);
                if !doc.set_materialized(f, generation, file.clone()) {
                    return;
                }
                doc.set_size_hint(f, counts_hint(counts, hunks));
                self.shared.counted[f as usize].store(true, Ordering::Relaxed);
                self.stats.token_cache_hits += u64::from(cached);
                let w = &mut self.work[f as usize];
                w.counts = Some(counts);
                w.languages = languages;
                w.syntax = SIDES.map(|side| {
                    if large || file.tokens(side).is_some() {
                        Syntax::Done
                    } else {
                        Syntax::Unknown
                    }
                });
                out.relayout.push(f);
                out.grew = true;
                out.repaint = true;
            }
            Outcome::Binary => {
                doc.cancel_loading(f);
                self.shared.counted[f as usize].store(true, Ordering::Relaxed);
                out.binary.push(f);
                out.repaint = true;
            }
            Outcome::Failed(message) => {
                if doc.set_failed(f, generation, message) {
                    out.relayout.push(f);
                    out.repaint = true;
                }
            }
        }
    }

    /// Drops `f` from the active list once nothing runs for it.
    fn settle(&mut self, f: u32) {
        if !self.work[f as usize].busy() {
            self.active.retain(|&a| a != f);
        }
    }

    /// File `f` lost its data (evicted): forget its tokens' state.
    pub fn forget(&mut self, f: u32) {
        let w = &mut self.work[f as usize];
        for s in &mut w.syntax {
            if let Syntax::Pending(flight) = s {
                flight.cancel();
                self.stats.cancelled += 1;
            }
            *s = Syntax::Unknown;
        }
        w.languages = [None, None];
        self.settle(f);
    }

    /// A new syntax theme: tokens of the old one are dropped, and files in
    /// the window are highlighted again as they are scheduled.
    pub fn set_theme(&mut self, theme: Arc<SyntaxTheme>, doc: &mut Document) {
        let same = self.highlighter.theme().id() == theme.id();
        self.highlighter = Arc::new(Highlighter::new(theme));
        self.dirty = true;
        if same {
            return;
        }
        for f in 0..doc.len() {
            let w = &mut self.work[f as usize];
            for s in &mut w.syntax {
                if let Syntax::Pending(flight) = s {
                    flight.cancel();
                    self.stats.cancelled += 1;
                    *s = Syntax::Unknown;
                }
            }
            if let FileState::Materialized(file) = doc.state(f) {
                for side in SIDES {
                    if file.tokens(side).is_some() {
                        w.syntax[side_index(side)] = Syntax::Unknown;
                    }
                }
                if file.old_tokens.is_some() || file.new_tokens.is_some() {
                    let plain = Arc::new(file.with_tokens(None, None));
                    let generation = doc.generation(f);
                    doc.set_materialized(f, generation, plain);
                }
            }
            self.settle(f);
        }
    }

    /// Syntax turned on or off.
    pub fn set_syntax(&mut self, on: bool) {
        self.dirty = true;
        if on {
            return;
        }
        for &f in &self.active.clone() {
            for s in &mut self.work[f as usize].syntax {
                if let Syntax::Pending(flight) = s {
                    flight.cancel();
                    self.stats.cancelled += 1;
                    *s = Syntax::Unknown;
                }
            }
            self.settle(f);
        }
    }

    /// Diff options changed: every loaded file goes back to unloaded (heights
    /// kept) and reloads when near the viewport; counts are recomputed.
    /// Returns the files reset.
    pub fn reload_all(&mut self, doc: &mut Document, diff: DiffOptions) -> Vec<u32> {
        self.cancel_all(doc);
        self.epoch += 1;
        self.dirty = true;
        let mut reset = Vec::new();
        for f in 0..doc.len() {
            if matches!(
                doc.state(f),
                FileState::Materialized(_) | FileState::Loading { .. } | FileState::Failed(_)
            ) {
                doc.begin_loading(f);
                doc.cancel_loading(f);
                reset.push(f);
            }
            let w = &mut self.work[f as usize];
            w.syntax = [Syntax::Unknown, Syntax::Unknown];
            w.languages = [None, None];
            let change = &self.shared.changes[f as usize];
            if needs_counts(change) {
                w.counts = None;
                self.shared.counted[f as usize].store(false, Ordering::Relaxed);
            }
        }
        if let Some(pass) = &mut lock(&self.shared.queue).pass {
            pass.next_count = 0;
            pass.epoch = self.epoch;
            pass.diff = diff;
        }
        reset
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        for w in &self.work {
            if let Some(flight) = &w.load {
                flight.cancel();
            }
            for s in &w.syntax {
                if let Syntax::Pending(flight) = s {
                    flight.cancel();
                }
            }
        }
        let mut q = lock(&self.shared.queue);
        q.urgent.clear();
        q.pass = None;
    }
}

fn counts_hint(counts: FileCounts, hunks: u32) -> SizeHint {
    SizeHint::Counts {
        additions: counts.additions,
        deletions: counts.deletions,
        hunks: Some(hunks),
    }
}

/// Sort key of a job for file `f`: visible files first (loads, then
/// highlights, top to bottom), then by pixel distance from the viewport
/// `[top, bottom)` (below before above on ties, a load before a highlight).
fn priority(
    f: u32,
    highlight: bool,
    doc: &Document,
    visible: &Range<u32>,
    top: f64,
    bottom: f64,
) -> (u8, u64, u64) {
    let stage = u64::from(highlight);
    if visible.contains(&f) {
        return (0, stage, u64::from(f));
    }
    let start = doc.file_top(f);
    let end = start + f64::from(doc.file_height(f));
    let (distance, above) = if end <= top {
        (top - end, 1)
    } else {
        ((start - bottom).max(0.0), 0)
    };
    (1, distance.round() as u64, stage << 1 | above)
}
