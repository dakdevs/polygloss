//! The live watcher (design §10 "Watch" and "Debounce", library-choices §7):
//! `notify` (FSEvents) through `notify-debouncer-full` with `NoCache` (no
//! tree walk on `watch()`, so a Linux-size worktree starts instantly),
//! 200 ms trailing per path.
//!
//! A live review watches its worktree (recursive) plus its git dir and the
//! common dir (outside the worktree for linked worktrees); a compare review
//! watches only the two git dirs, for its refs (OQ-27). Events are filtered
//! on the watcher's thread before they reach the app ([`PathFilter`]):
//!
//! - inside the git dirs only `HEAD`, `index`, `refs/**` and `packed-refs`
//!   count (so `.git/objects/**`, `.git/logs/**`, other worktrees' admin
//!   dirs, `FETCH_HEAD` and the rest never trigger), and never `*.lock`
//!   files there (git's lock files; a worktree's `Cargo.lock` or
//!   `bun.lock` is content and does count);
//! - Polygloss's scratch store never counts;
//! - worktree paths inside a nested `.git` never count, and gitignored ones
//!   are dropped in one `git check-ignore -z --stdin` per batch.
//!
//! A batch with anything left (or one the backend says needs a rescan, or
//! an error: events may have been lost) wakes the app once: the
//! [`LiveWatcher`] entity emits [`WatcherChanged`] and the review tab
//! recomputes in the background ([`super::recompute`]).
//!
//! Max wait: `notify-debouncer-full` emits each path's events once they are
//! 200 ms old, even while other writes keep coming (it has no global
//! trailing window), so a burst reaches the app at most ≈ 250 ms (one 50 ms
//! tick) after each write, well inside design §10's 1 s max wait.
//! Recomputes are serialized ([`super::Live`]): a batch that arrives during
//! one runs another once it ends, so a burst costs at most two.
//!
//! Waking follows `settings::init` (T3.1): with
//! [`crate::app_state::watchers_wake_the_app`] a task awaits the channel, so
//! an idle app never wakes up; without it (GPUI's test scheduler) the
//! channel is polled every [`crate::app_state::WATCHER_POLL`] with a
//! non-waking `try_recv`. Watchers keep running while the app is unfocused.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, WeakEntity};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    DebounceEventHandler, DebounceEventResult, DebouncedEvent, Debouncer, NoCache,
    new_debouncer_opt,
};
use polygloss_core::git::{Git, RepoInfo};

use crate::app_state::{AppState, WATCHER_POLL, watchers_wake_the_app};
use crate::settings::loader::take_changes;

/// Trailing debounce per path (design §10, provisional; plan OQ-P15).
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Design §10's max wait: a write reaches the app within this even during
/// a burst (see the module docs).
pub const MAX_WAIT: Duration = Duration::from_secs(1);

/// What a watcher looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchScope {
    /// A live review: the worktree and the git dirs.
    Worktree,
    /// A compare review: only refs (and `HEAD`) in the git dirs.
    Refs,
}

/// Where a changed path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Noise: never triggers a recompute.
    Drop,
    /// A git dir file that matters (`HEAD`, `index`, a ref).
    Git,
    /// A worktree file; it counts unless gitignored.
    Worktree,
}

/// Classifies changed paths for one watcher (see the module docs).
#[derive(Debug, Clone)]
pub struct PathFilter {
    scope: WatchScope,
    worktree: Option<PathBuf>,
    git_dir: PathBuf,
    common_dir: PathBuf,
    scratch_dir: PathBuf,
}

impl PathFilter {
    /// The filter for `repo`: a live review of `worktree` when given, else
    /// the refs of a compare review. `scratch_dir` is
    /// `DataPaths.scratch_dir`.
    pub fn new(repo: &RepoInfo, worktree: Option<&Path>, scratch_dir: &Path) -> PathFilter {
        PathFilter {
            scope: if worktree.is_some() {
                WatchScope::Worktree
            } else {
                WatchScope::Refs
            },
            worktree: worktree.map(canonical),
            git_dir: canonical(&repo.git_dir),
            common_dir: canonical(&repo.common_dir),
            scratch_dir: canonical(scratch_dir),
        }
    }

    pub fn scope(&self) -> WatchScope {
        self.scope
    }

    /// The directories to watch (recursively), none inside another.
    pub fn roots(&self) -> Vec<PathBuf> {
        let mut all: Vec<PathBuf> = self
            .worktree
            .iter()
            .chain([&self.git_dir, &self.common_dir])
            .cloned()
            .collect();
        all.sort();
        let mut roots: Vec<PathBuf> = Vec::new();
        for dir in all {
            if !roots.iter().any(|r| dir.starts_with(r)) {
                roots.push(dir);
            }
        }
        roots
    }

    /// Where `path` (absolute, as the backend reports it) is.
    pub fn classify(&self, path: &Path) -> Verdict {
        if path.starts_with(&self.scratch_dir) {
            return Verdict::Drop;
        }
        let in_git_dir = path.strip_prefix(&self.git_dir).ok();
        let in_common = path.strip_prefix(&self.common_dir).ok();
        if in_git_dir.is_some() || in_common.is_some() {
            if path.extension() == Some(OsStr::new("lock")) {
                return Verdict::Drop;
            }
            let is = |rel: Option<&Path>, name: &str| rel.is_some_and(|r| r == Path::new(name));
            let head = is(in_git_dir, "HEAD");
            let index = is(in_git_dir, "index");
            let refs =
                is(in_common, "packed-refs") || in_common.is_some_and(|r| r.starts_with("refs"));
            return match self.scope {
                WatchScope::Worktree if head || index || refs => Verdict::Git,
                WatchScope::Refs if head || refs => Verdict::Git,
                _ => Verdict::Drop,
            };
        }
        let Some(worktree) = &self.worktree else {
            return Verdict::Drop;
        };
        match path.strip_prefix(worktree) {
            Ok(rel)
                if !rel
                    .components()
                    .any(|c| c == Component::Normal(OsStr::new(".git"))) =>
            {
                Verdict::Worktree
            }
            _ => Verdict::Drop,
        }
    }

    /// Whether a debounced batch holds a change that matters: a rescan, a
    /// git file that counts, or a worktree path git does not ignore.
    pub fn relevant(&self, events: &[DebouncedEvent]) -> bool {
        let mut worktree_paths = Vec::new();
        for event in events {
            if event.need_rescan() {
                return true;
            }
            for path in &event.paths {
                match self.classify(path) {
                    Verdict::Git => return true,
                    Verdict::Worktree => worktree_paths.push(path.clone()),
                    Verdict::Drop => {}
                }
            }
        }
        match &self.worktree {
            Some(worktree) if !worktree_paths.is_empty() => {
                !drop_ignored(worktree, worktree_paths).is_empty()
            }
            _ => false,
        }
    }
}

/// `paths` (inside `worktree`) minus those git ignores, in one
/// `git check-ignore -z --stdin`. The worktree root itself is kept. When
/// git fails every path is kept (a spurious recompute is harmless).
pub fn drop_ignored(worktree: &Path, paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut rels: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(paths.len());
    let mut kept = Vec::new();
    for path in paths {
        match path.strip_prefix(worktree) {
            Ok(rel) if !rel.as_os_str().is_empty() => {
                let rel = rel.as_os_str().as_bytes().to_vec();
                rels.push((path, rel));
            }
            _ => kept.push(path),
        }
    }
    if rels.is_empty() {
        return kept;
    }
    let mut stdin = Vec::new();
    for (_, rel) in &rels {
        stdin.extend_from_slice(rel);
        stdin.push(0);
    }
    let args = [
        OsStr::new("check-ignore"),
        OsStr::new("-z"),
        OsStr::new("--stdin"),
    ];
    let ignored: std::collections::HashSet<Vec<u8>> =
        match Git::new(worktree).run_stdin(&args, &stdin) {
            // 0: some are ignored; 1: none is.
            Ok(out) if out.code == Some(0) => out
                .stdout
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(<[u8]>::to_vec)
                .collect(),
            Ok(out) if out.code == Some(1) => Default::default(),
            Ok(out) => {
                tracing::debug!(
                    "git check-ignore exited {:?}: {}",
                    out.code,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                Default::default()
            }
            Err(e) => {
                tracing::debug!("git check-ignore: {e}");
                Default::default()
            }
        };
    kept.extend(
        rels.into_iter()
            .filter(|(_, rel)| !ignored.contains(rel))
            .map(|(path, _)| path),
    );
    kept
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Emitted by [`LiveWatcher`] after a batch of relevant changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatcherChanged;

/// A running watcher (a GPUI entity; dropping it stops the watch).
pub struct LiveWatcher {
    filter: PathFilter,
    /// Relevant batches seen so far.
    batches: u64,
    /// Why nothing is watched, if the watch could not start.
    error: Option<String>,
    _debouncer: Option<Debouncer<RecommendedWatcher, NoCache>>,
}

impl EventEmitter<WatcherChanged> for LiveWatcher {}

impl LiveWatcher {
    /// Watches the live review of `worktree` in `repo`: the worktree
    /// (recursive), `--git-dir` and `--git-common-dir`.
    pub fn start(repo: &RepoInfo, worktree: &Path, cx: &mut App) -> Entity<LiveWatcher> {
        let scratch = AppState::global(cx).paths.scratch_dir.clone();
        LiveWatcher::with_filter(PathFilter::new(repo, Some(worktree), &scratch), cx)
    }

    /// Watches the refs of a compare review in `repo` (OQ-27).
    pub fn start_refs(repo: &RepoInfo, cx: &mut App) -> Entity<LiveWatcher> {
        let scratch = AppState::global(cx).paths.scratch_dir.clone();
        LiveWatcher::with_filter(PathFilter::new(repo, None, &scratch), cx)
    }

    fn with_filter(filter: PathFilter, cx: &mut App) -> Entity<LiveWatcher> {
        let (debouncer, rx, error) = match watch(&filter) {
            Ok((debouncer, rx)) => (Some(debouncer), Some(rx), None),
            Err(e) => {
                let roots = filter.roots();
                tracing::warn!("not watching {roots:?}: {e}");
                (None, None, Some(e.to_string()))
            }
        };
        let entity = cx.new(|_| LiveWatcher {
            filter,
            batches: 0,
            error,
            _debouncer: debouncer,
        });
        if let Some(rx) = rx {
            forward(rx, entity.downgrade(), cx);
        }
        entity
    }

    /// The filter in use.
    pub fn filter(&self) -> &PathFilter {
        &self.filter
    }

    /// Relevant batches seen so far.
    pub fn batches(&self) -> u64 {
        self.batches
    }

    /// Why nothing is watched, if the watch could not start.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.batches += 1;
        cx.emit(WatcherChanged);
    }
}

/// Starts the debouncer on the filter's roots.
fn watch(
    filter: &PathFilter,
) -> notify::Result<(
    Debouncer<RecommendedWatcher, NoCache>,
    UnboundedReceiver<()>,
)> {
    let (tx, rx) = unbounded();
    let handler = Batches {
        filter: filter.clone(),
        tx,
    };
    let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
        DEBOUNCE,
        None,
        handler,
        NoCache,
        notify::Config::default(),
    )?;
    for root in filter.roots() {
        debouncer.watch(&root, RecursiveMode::Recursive)?;
    }
    Ok((debouncer, rx))
}

/// Filters debounced batches on the watcher's thread and forwards one
/// notice per relevant batch.
struct Batches {
    filter: PathFilter,
    tx: UnboundedSender<()>,
}

impl DebounceEventHandler for Batches {
    fn handle_event(&mut self, result: DebounceEventResult) {
        let relevant = match &result {
            Ok(events) => self.filter.relevant(events),
            // Events may have been lost: recompute to be safe.
            Err(_) => true,
        };
        if relevant {
            let _ = self.tx.unbounded_send(());
        }
    }
}

/// Delivers the watcher thread's notices to `watcher` on the main thread
/// (see the module docs) until the watcher is dropped.
fn forward(mut rx: UnboundedReceiver<()>, watcher: WeakEntity<LiveWatcher>, cx: &mut App) {
    if watchers_wake_the_app(cx) {
        cx.spawn(async move |cx: &mut AsyncApp| {
            while rx.next().await.is_some() {
                // A burst of notices is one recompute.
                take_changes(&mut rx);
                if watcher.update(cx, |w, cx| w.changed(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    } else {
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(WATCHER_POLL).await;
                let changed = match take_changes(&mut rx) {
                    Some(changed) => changed,
                    None => break,
                };
                let alive = if changed {
                    watcher.update(cx, |w, cx| w.changed(cx)).is_ok()
                } else {
                    watcher.upgrade().is_some()
                };
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }
}
