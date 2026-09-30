//! `watcher-banner`: `watcher_banner_ms` (plan T2.9, design §10 and §12.1:
//! one file write, no further writes → the live banner visible, < 500 ms on
//! the typical corpus; p95 over [`WRITES`] writes).
//!
//! The run makes a private working tree of the corpus (a new repo whose
//! objects are the corpus's, through `objects/info/alternates`, checked out
//! at the corpus head on branch `perf`) and opens it through the app's real
//! startup as a live review with a fixed base (the corpus base, or its
//! merge base with the head for a three-dot corpus), so the review shows
//! the corpus diff. Once the tab has settled and its watcher runs, each
//! write appends one line to one of the diff's text files (a single
//! `write`); the sample is the time from just before that write to the end
//! of the first frame painted with the "N files changed" banner (frames are
//! stamped when the viewport reports them, after paint, like T2.9's). Then
//! the run refreshes (`R`), waits for the tab to settle and for 300 ms of
//! quiet, and writes again. A write whose banner does not show within 5 s
//! counts as 5 s (`info.timeouts`).

use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{AnyWindowHandle, App, AsyncApp, Entity};
use polygloss_core::git::{Git, Since, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, FileKind, FileStatus};
use polygloss_viewport::ViewportEvent;
use serde_json::json;

use crate::live;
use crate::perf::{
    Clock, CorpusSpec, PerfArgs, ScenarioResult, finish, max_rss_mb, ms, p95, run_dir,
};
use crate::review_tab::{self, BannerKind, ReviewTab};
use crate::settings::SettingsStore;
use crate::settings::model::LayoutSetting;
use crate::startup::{self, Launch};

/// Writes measured (T2.9: p95 over 20).
pub const WRITES: usize = 20;

/// Files the writes rotate over.
const EDITED_FILES: usize = 5;

/// Longest wait for one write's banner.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Quiet time before each write (the previous refresh's frames and any
/// late events have passed).
const QUIET: Duration = Duration::from_millis(300);

/// Longest wait for the tab to open and settle, and for a refresh.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the driver checks the run's state (the samples themselves
/// are stamped by the frame callback, not by this poll).
const POLL: Duration = Duration::from_millis(5);

/// A live working tree of the corpus and the base to diff it against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCorpus {
    pub worktree: PathBuf,
    /// The base commit (full id).
    pub base: String,
}

/// Makes `dir` a working tree of `spec`'s repo at its head: `git init`, the
/// corpus's object store as an alternate (nothing is copied or fetched),
/// `checkout -b perf <head>`. The base is `spec.base`, or its merge base
/// with the head for a three-dot corpus.
pub fn make_worktree(spec: &CorpusSpec, dir: &Path) -> anyhow::Result<LiveCorpus> {
    let corpus = Git::new(&spec.repo);
    let rev = |r: &str| -> anyhow::Result<String> {
        let spec = format!("{r}^{{commit}}");
        let out = corpus
            .output(&[
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--end-of-options"),
                OsStr::new(&spec),
            ])
            .with_context(|| format!("resolving {r} in the corpus"))?;
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    };
    let head = rev(&spec.head)?;
    let base = if spec.direct {
        rev(&spec.base)?
    } else {
        let base = rev(&spec.base)?;
        let out = corpus
            .output(&[
                OsStr::new("merge-base"),
                OsStr::new(&base),
                OsStr::new(&head),
            ])
            .context("the corpus's merge base")?;
        String::from_utf8_lossy(&out).trim().to_owned()
    };
    let common = corpus
        .output(&[
            OsStr::new("rev-parse"),
            OsStr::new("--path-format=absolute"),
            OsStr::new("--git-common-dir"),
        ])
        .context("the corpus's git dir")?;
    let objects = PathBuf::from(String::from_utf8_lossy(&common).trim()).join("objects");

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let wt = Git::new(dir);
    wt.output(&[OsStr::new("init"), OsStr::new("-q")])
        .context("git init")?;
    let info = dir.join(".git/objects/info");
    std::fs::create_dir_all(&info)?;
    std::fs::write(info.join("alternates"), format!("{}\n", objects.display()))
        .context("writing the alternates file")?;
    wt.output(&[
        OsStr::new("checkout"),
        OsStr::new("-q"),
        OsStr::new("-b"),
        OsStr::new("perf"),
        OsStr::new(&head),
    ])
    .context("checking out the corpus head")?;
    Ok(LiveCorpus {
        worktree: std::fs::canonicalize(dir)?,
        base,
    })
}

/// The files the writes go to: the diff's text files that exist in the
/// working tree (not deleted, not generated), in diff order.
pub fn edited_files(files: &[FileChange], worktree: &Path) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|f| f.kind == FileKind::Text && f.status != FileStatus::Deleted && !f.generated)
        .filter_map(|f| f.new_path.as_ref())
        .filter(|p| !p.escaped)
        .map(|p| worktree.join(&p.text))
        .filter(|p| p.is_file())
        .take(EDITED_FILES)
        .collect()
}

/// Appends one line to `path` in a single `write` (a save).
fn append_line(path: &Path, n: usize) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(format!("// polygloss perf edit {n}\n").as_bytes())
}

/// What the run saw so far.
#[derive(Default)]
struct Run {
    files: usize,
    /// The last painted frame showed no loading rows and no plain rows.
    settled: bool,
    /// A write is waiting for its banner since this instant.
    waiting: Option<Instant>,
    /// The end of the first frame painted with the banner.
    banner_painted: Option<Instant>,
}

pub fn run(args: PerfArgs, spec: CorpusSpec, clock: Clock) -> ExitCode {
    let (root, paths) = match run_dir() {
        Ok(dir) => dir,
        Err(err) => finish(Err(err), &args),
    };
    let corpus = match make_worktree(&spec, &root.join("worktree")) {
        Ok(corpus) => corpus,
        Err(err) => finish(Err(err.context("making the live working tree")), &args),
    };
    let req = OpenRequest {
        worktree: corpus.worktree.clone(),
        source: Source::Live {
            since: Since::Commit(corpus.base.clone()),
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let args = Rc::new(args);
    let state = Rc::new(RefCell::new(Run::default()));
    let (before_args, before_state) = (args.clone(), state.clone());
    let (after_args, after_state) = (args.clone(), state.clone());
    startup::run(Launch {
        urls: Vec::new(),
        open: Some(req),
        paths: Some(paths),
        clock,
        before_window: Some(Box::new(move |cx: &mut App| {
            let mut settings = SettingsStore::global(cx).settings().clone();
            settings.diff.layout = match before_args.layout {
                Layout::Split => LayoutSetting::Split,
                Layout::Unified => LayoutSetting::Unified,
            };
            SettingsStore::set(settings, cx);
            let state = before_state.clone();
            review_tab::on_new_tab(cx, move |tab, _window, cx| {
                {
                    let mut s = state.borrow_mut();
                    s.files = tab.read(cx).opened.files.len();
                }
                let viewport = tab.read(cx).viewport.clone();
                let (state, tab) = (state.clone(), tab.downgrade());
                cx.subscribe(&viewport, move |_, event: &ViewportEvent, cx| {
                    let ViewportEvent::FrameStats(stats) = event else {
                        return;
                    };
                    let painted = Instant::now();
                    let banner = tab.upgrade().is_some_and(|t| has_banner(&t, cx));
                    let mut s = state.borrow_mut();
                    s.settled = stats.loading_rows == 0 && stats.unhighlighted_rows == 0;
                    if s.waiting.is_some() && s.banner_painted.is_none() && banner {
                        s.banner_painted = Some(painted);
                    }
                })
                .detach();
            });
        })),
        after_launch: Some(Box::new(move |launched, cx: &mut App| {
            let args = after_args.clone();
            let Some(opening) = launched.opening else {
                finish(Err(anyhow::anyhow!("the window did not open")), &args);
            };
            let window = launched.window;
            let state = after_state.clone();
            let edited = corpus.worktree.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let tab = match opening.await {
                    Ok(tab) => tab,
                    Err(err) => finish(Err(err.context("opening the live corpus")), &args),
                };
                let outcome = drive(&tab, window, &state, &edited, cx).await;
                let result = outcome.map(|samples| result(&args, &state.borrow(), samples, clock));
                finish(result, &args)
            })
            .detach();
        })),
    })
}

fn has_banner(tab: &Entity<ReviewTab>, cx: &App) -> bool {
    tab.read(cx)
        .banners
        .read(cx)
        .banners()
        .iter()
        .any(|(k, _)| *k == BannerKind::LiveChanges)
}

/// Samples of one run and how many writes timed out.
struct Samples {
    write_ms: Vec<f64>,
    timeouts: usize,
    edited: usize,
}

/// Waits until `cond` holds, polling every [`POLL`].
async fn wait(
    cx: &mut AsyncApp,
    timeout: Duration,
    mut cond: impl FnMut(&mut AsyncApp) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond(cx) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        cx.background_executor().timer(POLL).await;
    }
}

/// Whether the tab is idle: settled, its watcher running, no banner, no
/// recompute or refresh running.
fn idle(tab: &Entity<ReviewTab>, state: &Rc<RefCell<Run>>, cx: &mut AsyncApp) -> bool {
    let settled = state.borrow().settled;
    settled
        && cx.update(|cx| {
            let t = tab.read(cx);
            let watching = live::live(t).is_some_and(|l| l.watcher().is_some() && !l.busy());
            watching && !has_banner(tab, cx)
        })
}

async fn drive(
    tab: &Entity<ReviewTab>,
    window: AnyWindowHandle,
    state: &Rc<RefCell<Run>>,
    worktree: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<Samples> {
    let files = cx.update(|cx| edited_files(&tab.read(cx).opened.files, worktree));
    if files.is_empty() {
        anyhow::bail!("the corpus diff has no text file to edit");
    }
    let mut samples = Samples {
        write_ms: Vec::with_capacity(WRITES),
        timeouts: 0,
        edited: files.len(),
    };
    for n in 0..WRITES {
        if !wait(cx, SETTLE_TIMEOUT, |cx| idle(tab, state, cx)).await {
            anyhow::bail!("the review did not settle before write {n}");
        }
        cx.background_executor().timer(QUIET).await;
        if !idle(tab, state, cx) {
            anyhow::bail!("the review changed without a write before write {n}");
        }
        let path = &files[n % files.len()];
        let start = Instant::now();
        {
            let mut s = state.borrow_mut();
            s.waiting = Some(start);
            s.banner_painted = None;
        }
        append_line(path, n).with_context(|| format!("writing {}", path.display()))?;
        let shown = wait(cx, WRITE_TIMEOUT, |_| {
            state.borrow().banner_painted.is_some()
        })
        .await;
        let sample = {
            let mut s = state.borrow_mut();
            s.waiting = None;
            match s.banner_painted.take() {
                Some(at) if shown => ms(at.saturating_duration_since(start)),
                _ => {
                    samples.timeouts += 1;
                    ms(WRITE_TIMEOUT)
                }
            }
        };
        samples.write_ms.push(sample);
        window.update(cx, |_, window, cx| {
            tab.update(cx, |t, cx| live::refresh_tab(t, window, cx));
        })?;
    }
    Ok(samples)
}

fn result(args: &PerfArgs, run: &Run, samples: Samples, clock: Clock) -> ScenarioResult {
    let mut result = ScenarioResult::new(args);
    result
        .metrics
        .insert("watcher_banner_ms".into(), json!(p95(&samples.write_ms)));
    result
        .samples
        .insert("write_ms".into(), json!(samples.write_ms));
    result.info.insert("files".into(), json!(run.files));
    result.info.insert("writes".into(), json!(WRITES));
    result
        .info
        .insert("edited_files".into(), json!(samples.edited));
    result
        .info
        .insert("timeouts".into(), json!(samples.timeouts));
    result.info.insert("max_rss_mb".into(), json!(max_rss_mb()));
    result.info.insert(
        "process_start".into(),
        json!(if clock.from_kernel { "kernel" } else { "main" }),
    );
    result
}
