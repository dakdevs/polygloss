//! `comment-roundtrip`: `comment_repaint_ms` with real thread blocks (plan
//! T2.9 from M3, design §12.1 "comment round-trip < 50 ms"): save a draft
//! and resolve a thread through `Core` → the next completed frame that
//! shows it, p95 over [`OPS`] ops of each.
//!
//! The run opens the corpus through the app's real startup (a compare
//! review, like `open`), waits for the tab to settle, then:
//!
//! 1. **Drafts** ([`OPS`]): a line composer (T3.10) opens on the code line
//!    nearest the middle of the viewport without a thread yet
//!    ([`visible_line`], T2.9's `blocks` placement), holding a short
//!    comment; once it is painted and the tab is quiet, the sample runs from
//!    `⌘⏎`'s save (`composer::save`: `Core::create_thread` on the
//!    background executor, then the threads' reload) to the end of the
//!    first frame painted with the new thread's block (and the composer
//!    gone).
//! 2. The drafts are submitted (not measured), which publishes them.
//! 3. **Resolves** ([`OPS`]): each thread in turn, from
//!    `composer::set_resolved` (`Core::set_resolved`, then the reload) to
//!    the end of the first frame painted with it resolved (its block
//!    collapsed to a chip).
//!
//! `comment_repaint_ms` is the larger of the two p95s (both must hold the
//! budget); `info` has each. An op whose frame does not come within 5 s
//! counts as 5 s (`info.timeouts`).

use std::cell::RefCell;
use std::collections::HashSet;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, AsyncApp, Entity};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{OpenRequest, ThreadStatus, Verdict};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{BodyRow, Document, ViewportEvent};
use serde_json::json;

use crate::composer::{self, ComposerKey};
use crate::perf::{
    Clock, CorpusSpec, PerfArgs, ScenarioResult, finish, max_rss_mb, ms, p95, run_dir,
};
use crate::review_tab::{self, ReviewTab};
use crate::settings::SettingsStore;
use crate::settings::model::LayoutSetting;
use crate::startup::{self, Launch};
use crate::threads;

/// Ops of each kind (T2.9: p95 over 20).
pub const OPS: usize = 20;

/// Longest wait for one op's frame.
const OP_TIMEOUT: Duration = Duration::from_secs(5);

/// No frame for this long counts as quiet.
const QUIET: Duration = Duration::from_millis(150);

/// Longest wait for the tab to open and settle.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the driver checks the run's state (samples are stamped by the
/// frame callback, not by this poll).
const POLL: Duration = Duration::from_millis(2);

/// The comment each draft holds.
pub const COMMENT: &str = "This allocates on the hot path: could the label be cached per row?";

/// What the frame callback looks for, and what it saw.
#[derive(Default)]
struct Run {
    files: usize,
    /// The last frame had no loading and no plain rows.
    settled: bool,
    last_frame: Option<Instant>,
    /// The op being measured: the condition that ends it.
    waiting: Option<Waiting>,
    /// The end of the first frame painted with the op done.
    done_at: Option<Instant>,
}

/// What ends an op.
#[derive(Debug, Clone)]
enum Waiting {
    /// One thread beyond the `known` ones, with its block in the viewport,
    /// and the composer `key` closed.
    Draft {
        known: HashSet<String>,
        key: ComposerKey,
    },
    /// Thread `id` resolved.
    Resolve { id: String },
}

/// Where to put a comment so it shows: the code line nearest the middle
/// of the viewport (at or below it first, then above) that is not in
/// `used`, in whichever visible, laid-out file has one (T2.9's `blocks`
/// placement, lines only). `(file, side, line)`, 0-based.
pub fn visible_line(doc: &Document, used: &HashSet<(u32, Side, u32)>) -> Option<(u32, Side, u32)> {
    if doc.is_empty() {
        return None;
    }
    let height = f64::from(doc.viewport_height());
    let top = doc.scroll_top();
    let (bottom, mid) = (top + height, top + height / 2.0);
    let header = f64::from(doc.metrics().header_height);
    let (mid_file, _) = doc.file_at_offset(mid);
    let free = |f: u32| move |l: &(Side, u32)| !used.contains(&(f, l.0, l.1));
    let last = doc.file_at_offset(bottom).0;
    for f in mid_file..=last {
        if let Some((s, l)) = lines_in(doc, f, mid, bottom).into_iter().find(free(f)) {
            return Some((f, s, l));
        }
    }
    let first = doc.file_at_offset(top).0;
    for f in (first..=mid_file).rev() {
        // Rows right under the (pinned) header at the top are covered.
        let lines = lines_in(doc, f, top + header, mid);
        if let Some((s, l)) = lines.into_iter().rev().find(free(f)) {
            return Some((f, s, l));
        }
    }
    None
}

/// The code lines of file `f` whose rows start within `[from, to)` in
/// document coordinates, top to bottom: the new side where the row has
/// one, else the old.
fn lines_in(doc: &Document, f: u32, from: f64, to: f64) -> Vec<(Side, u32)> {
    let Some(layout) = doc.file_layout(f).filter(|_| !doc.is_collapsed(f)) else {
        return Vec::new();
    };
    let body_top = doc.body_top(f);
    let lo = (from - body_top).max(0.0);
    let hi = (to - body_top).min(layout.height());
    if lo >= hi || layout.is_empty() {
        return Vec::new();
    }
    let (first, _) = layout.row_at(lo);
    (first..layout.len())
        .take_while(|&i| layout.row_top(i) < hi)
        .filter_map(|i| match layout.rows()[i] {
            BodyRow::Line {
                new: Some(line), ..
            } => Some((Side::New, line)),
            BodyRow::Line {
                old: Some(line), ..
            } => Some((Side::Old, line)),
            _ => None,
        })
        .collect()
}

pub fn run(args: PerfArgs, spec: CorpusSpec, clock: Clock) -> ExitCode {
    let paths = match run_dir() {
        Ok((_, paths)) => paths,
        Err(err) => finish(Err(err), &args),
    };
    let req = OpenRequest {
        worktree: spec.repo.clone(),
        source: Source::Compare {
            base: spec.base.clone(),
            head: spec.head.clone(),
            mode: if spec.direct {
                CompareMode::Direct
            } else {
                CompareMode::ThreeDot
            },
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
                state.borrow_mut().files = tab.read(cx).opened.files.len();
                let viewport = tab.read(cx).viewport.clone();
                let (state, tab) = (state.clone(), tab.downgrade());
                cx.subscribe(&viewport, move |_, event: &ViewportEvent, cx| {
                    let ViewportEvent::FrameStats(stats) = event else {
                        return;
                    };
                    let painted = Instant::now();
                    let waiting = state.borrow().waiting.clone();
                    let done = match (&waiting, tab.upgrade()) {
                        (Some(w), Some(tab)) => op_done(&tab, w, cx),
                        _ => false,
                    };
                    let mut s = state.borrow_mut();
                    s.settled = stats.loading_rows == 0 && stats.unhighlighted_rows == 0;
                    s.last_frame = Some(painted);
                    if done && s.done_at.is_none() {
                        s.done_at = Some(painted);
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
            cx.spawn(async move |cx: &mut AsyncApp| {
                let tab = match opening.await {
                    Ok(tab) => tab,
                    Err(err) => finish(Err(err.context("opening the corpus")), &args),
                };
                let outcome = drive(&tab, window, &state, cx).await;
                let result = outcome.map(|samples| result(&args, &state.borrow(), samples, clock));
                finish(result, &args)
            })
            .detach();
        })),
    })
}

/// Whether the op `w` shows in the tab's model and viewport.
fn op_done(tab: &Entity<ReviewTab>, w: &Waiting, cx: &App) -> bool {
    let t = tab.read(cx);
    let Some(model) = threads::threads(t) else {
        return false;
    };
    let m = model.read(cx);
    match w {
        Waiting::Draft { known, key } => {
            let closed = composer::composer(t, key, cx).is_none();
            let mut new = m.threads().filter(|t| !known.contains(&t.id));
            let (Some(saved), None) = (new.next(), new.next()) else {
                return false;
            };
            let block = threads::placement::block_id(&saved.id);
            let doc = t.viewport.read(cx).document();
            closed
                && m.threads().count() == known.len() + 1
                && (0..doc.len()).any(|f| doc.blocks(f).iter().any(|b| b.id == block))
        }
        Waiting::Resolve { id } => m
            .thread(id)
            .is_some_and(|t| t.status == ThreadStatus::Resolved),
    }
}

/// Samples of one run.
struct Samples {
    draft_ms: Vec<f64>,
    resolve_ms: Vec<f64>,
    timeouts: usize,
    placed: Vec<serde_json::Value>,
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

/// Settled, the threads loaded, and no frame for [`QUIET`].
async fn settle(
    tab: &Entity<ReviewTab>,
    state: &Rc<RefCell<Run>>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let ok = wait(cx, SETTLE_TIMEOUT, |cx| {
        let loaded =
            cx.update(|cx| threads::threads(tab.read(cx)).is_some_and(|m| m.read(cx).is_loaded()));
        let s = state.borrow();
        loaded && s.settled && s.last_frame.is_some_and(|at| at.elapsed() >= QUIET)
    })
    .await;
    anyhow::ensure!(ok, "the review did not settle");
    Ok(())
}

/// Starts measuring op `w` (just before its call).
fn start(state: &Rc<RefCell<Run>>, w: Waiting) -> Instant {
    let mut s = state.borrow_mut();
    s.waiting = Some(w);
    s.done_at = None;
    Instant::now()
}

/// The op's sample: until its frame, or [`OP_TIMEOUT`].
async fn finish_op(
    state: &Rc<RefCell<Run>>,
    started: Instant,
    timeouts: &mut usize,
    cx: &mut AsyncApp,
) -> f64 {
    let shown = wait(cx, OP_TIMEOUT, |_| state.borrow().done_at.is_some()).await;
    let mut s = state.borrow_mut();
    s.waiting = None;
    match s.done_at.take() {
        Some(at) if shown => ms(at.saturating_duration_since(started)),
        _ => {
            *timeouts += 1;
            ms(OP_TIMEOUT)
        }
    }
}

async fn drive(
    tab: &Entity<ReviewTab>,
    window: AnyWindowHandle,
    state: &Rc<RefCell<Run>>,
    cx: &mut AsyncApp,
) -> anyhow::Result<Samples> {
    let mut samples = Samples {
        draft_ms: Vec::with_capacity(OPS),
        resolve_ms: Vec::with_capacity(OPS),
        timeouts: 0,
        placed: Vec::new(),
    };
    let mut used = HashSet::new();
    settle(tab, state, cx).await?;
    for _ in 0..OPS {
        // Threads fill the view: scroll a screen down for free lines.
        let mut target = None;
        for _ in 0..50 {
            target = cx.update(|cx| visible_line(tab.read(cx).viewport.read(cx).document(), &used));
            if target.is_some() {
                break;
            }
            tab.update(cx, |t, cx| {
                t.viewport.update(cx, |v, cx| {
                    let height = v.document().viewport_height();
                    v.scroll_by(height * 0.8, cx)
                })
            });
            settle(tab, state, cx).await?;
        }
        let (f, side, line) = target.context("no code line in view to comment on")?;
        used.insert((f, side, line));
        samples
            .placed
            .push(json!({ "file": f, "side": format!("{side:?}"), "line": line }));
        // The composer opens with the comment typed, and is painted.
        let key = window.update(cx, |_, window, cx| {
            tab.update(cx, |t, cx| {
                composer::open_line(t, f, side, line, line, window, cx);
                let path = t.opened.files[f as usize].display_path().to_owned();
                let key = ComposerKey::line(&path, side, line, line);
                if let Some(view) = composer::composer(t, &key, cx) {
                    let input = view.read(cx).input().clone();
                    input.update(cx, |s, cx| s.set_value(COMMENT, window, cx));
                }
                key
            })
        })?;
        settle(tab, state, cx).await?;
        let known: HashSet<String> = cx.update(|cx| {
            threads::threads(tab.read(cx)).map_or_else(HashSet::new, |m| {
                m.read(cx).threads().map(|t| t.id.clone()).collect()
            })
        });
        let started = start(
            state,
            Waiting::Draft {
                known,
                key: key.clone(),
            },
        );
        window.update(cx, |_, window, cx| {
            tab.update(cx, |t, cx| composer::save(t, &key, window, cx))
        })?;
        let sample = finish_op(state, started, &mut samples.timeouts, cx).await;
        samples.draft_ms.push(sample);
    }
    // Publish the drafts (humans cannot resolve drafts), then resolve each.
    let (core, review_id) = cx.update(|cx| {
        (
            crate::app_state::AppState::global(cx).core.clone(),
            tab.read(cx).review_id.clone(),
        )
    });
    cx.background_spawn(async move { core.submit_review(&review_id, Verdict::Comment, "", None) })
        .await
        .context("submitting the drafts")?;
    tab.update(cx, threads::reload);
    let ids: Vec<String> = {
        let ok = wait(cx, SETTLE_TIMEOUT, |cx| {
            cx.update(|cx| {
                threads::threads(tab.read(cx)).is_some_and(|m| {
                    let m = m.read(cx);
                    m.threads().count() >= OPS && m.threads().all(|t| !t.draft)
                })
            })
        })
        .await;
        anyhow::ensure!(ok, "the submitted threads did not load");
        cx.update(|cx| {
            threads::threads(tab.read(cx))
                .map(|m| m.read(cx).threads().map(|t| t.id.clone()).collect())
                .unwrap_or_default()
        })
    };
    for id in ids.into_iter().take(OPS) {
        settle(tab, state, cx).await?;
        let started = start(state, Waiting::Resolve { id: id.clone() });
        tab.update(cx, |t, cx| composer::set_resolved(t, &id, true, cx));
        let sample = finish_op(state, started, &mut samples.timeouts, cx).await;
        samples.resolve_ms.push(sample);
    }
    Ok(samples)
}

fn result(args: &PerfArgs, run: &Run, samples: Samples, clock: Clock) -> ScenarioResult {
    let draft = p95(&samples.draft_ms);
    let resolve = p95(&samples.resolve_ms);
    let worst = match (draft, resolve) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let mut result = ScenarioResult::new(args);
    result
        .metrics
        .insert("comment_repaint_ms".into(), json!(worst));
    result
        .samples
        .insert("draft_ms".into(), json!(samples.draft_ms));
    result
        .samples
        .insert("resolve_ms".into(), json!(samples.resolve_ms));
    result.info.insert("files".into(), json!(run.files));
    result.info.insert("ops".into(), json!(OPS));
    result.info.insert("draft_p95_ms".into(), json!(draft));
    result.info.insert("resolve_p95_ms".into(), json!(resolve));
    result
        .info
        .insert("timeouts".into(), json!(samples.timeouts));
    result.info.insert("placed".into(), json!(samples.placed));
    result.info.insert("max_rss_mb".into(), json!(max_rss_mb()));
    result.info.insert(
        "process_start".into(),
        json!(if clock.from_kernel { "kernel" } else { "main" }),
    );
    result
}
