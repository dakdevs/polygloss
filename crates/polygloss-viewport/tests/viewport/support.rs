//! Shared helpers for the viewport's GPUI tests: a per-test sandbox, an
//! in-memory `DiffProvider`, a window with a `DiffViewport` in it, and readers
//! for what it painted.
//!
//! Every test starts with `let _sb = sandbox();` (plan "Test hygiene"; `open`
//! checks it). GPUI's test platform shapes text with a no-op text system:
//! every char is `0.6 × font size` wide and the font is irrelevant, so
//! geometry is exact.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::{
    Entity, Hsla, Modifiers, Pixels, ScrollDelta, ScrollWheelEvent, Subscription, TestAppContext,
    VisualTestContext, point, px, size,
};
use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, Mode, ObjectFormat, Oid};
use polygloss_highlight::{Appearance, pierre_theme};
use polygloss_viewport::{
    DiffProvider, DiffViewport, FrameStats, LayoutMode, ViewportDebug, ViewportEvent,
    ViewportOptions, ViewportTheme,
};

/// Code font size the tests use: one char is `0.6 × 13 = 7.8` px wide.
pub const FONT_SIZE: f32 = 13.0;
pub const ADVANCE: f32 = 0.6 * FONT_SIZE;
/// Row height for `FONT_SIZE` (`round(13 × 1.5)`).
pub const ROW_H: f32 = 20.0;
pub const HEADER_H: f32 = 40.0;
/// A one-label body (`Binary file`, `Large diff`, a load error): 2.4 rows.
pub const PLACEHOLDER_H: f32 = 48.0;

/// Set while a [`Sandbox`] is alive; `open` refuses to run without it.
const SANDBOX_MARKER: &str = "POLYGLOSS_VIEWPORT_TEST_SANDBOX";

/// A per-test temp `HOME`, data dir, config and cache dirs and an empty git
/// config (plan "Test hygiene"). Keep it alive for the whole test; dropping it
/// deletes the temp dir. Env is not restored (one process per test).
pub struct Sandbox {
    _root: tempfile::TempDir,
    pub home: PathBuf,
}

/// Points `HOME`, `POLYGLOSS_DATA_DIR`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`
/// and git's global config at a fresh temp dir. Call it first in every test:
/// it sets **process** env, which is only sound because nextest runs every
/// test in its own process.
pub fn sandbox() -> Sandbox {
    let root = tempfile::Builder::new()
        .prefix("polygloss-viewport-test-")
        .tempdir()
        .expect("create sandbox temp dir");
    let base = std::fs::canonicalize(root.path()).expect("canonicalize sandbox");
    let home = base.join("home");
    let data = base.join("data");
    let config = home.join(".config");
    let cache = home.join(".cache");
    for dir in [&home, &data, &config, &cache] {
        std::fs::create_dir_all(dir).expect("create sandbox dir");
    }
    let git_config = home.join(".gitconfig-empty");
    std::fs::write(&git_config, "").expect("create empty git config");
    // SAFETY: nextest runs each test in its own process and the sandbox is
    // made first, before the test starts any thread that reads the env (the
    // GPUI test platform runs everything on the test thread).
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("POLYGLOSS_DATA_DIR", &data);
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_CACHE_HOME", &cache);
        std::env::set_var("GIT_CONFIG_GLOBAL", &git_config);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var(SANDBOX_MARKER, &home);
    }
    Sandbox { _root: root, home }
}

/// Panics unless the test made a [`Sandbox`] and `HOME` still points into it.
pub fn assert_sandboxed() {
    let marker = std::env::var_os(SANDBOX_MARKER);
    assert!(
        marker.is_some() && marker == std::env::var_os("HOME"),
        "start the test with `let _sb = sandbox();` (plan \"Test hygiene\")"
    );
}

/// One file of a synthetic diff. `None` sides are absent (added or deleted).
pub struct Spec {
    pub path: String,
    pub old: Option<String>,
    pub new: Option<String>,
    pub kind: FileKind,
    pub old_path: Option<String>,
}

impl Spec {
    pub fn modified(path: &str, old: &str, new: &str) -> Spec {
        Spec {
            path: path.to_owned(),
            old: Some(old.to_owned()),
            new: Some(new.to_owned()),
            kind: FileKind::Text,
            old_path: None,
        }
    }

    pub fn added(path: &str, new: &str) -> Spec {
        Spec {
            path: path.to_owned(),
            old: None,
            new: Some(new.to_owned()),
            kind: FileKind::Text,
            old_path: None,
        }
    }

    pub fn binary(path: &str) -> Spec {
        Spec {
            path: path.to_owned(),
            old: Some("\0old".to_owned()),
            new: Some("\0new".to_owned()),
            kind: FileKind::Binary,
            old_path: None,
        }
    }
}

/// An in-memory `DiffProvider` that counts blob loads and can fail them.
pub struct MemProvider {
    files: Arc<Vec<FileChange>>,
    blobs: HashMap<String, Arc<[u8]>>,
    /// Blobs that fail to load (see [`MemProvider::set_broken`]).
    broken: Mutex<HashSet<String>>,
    pub loads: AtomicUsize,
}

impl MemProvider {
    pub fn new(specs: Vec<Spec>) -> Arc<MemProvider> {
        let mut blobs = HashMap::new();
        let mut next = 1u64;
        let mut blob = |text: &Option<String>| match text {
            Some(t) => {
                let oid = Oid::parse(&format!("{next:040x}"), ObjectFormat::Sha1).unwrap();
                next += 1;
                blobs.insert(oid.as_str().to_owned(), Arc::<[u8]>::from(t.as_bytes()));
                oid
            }
            None => Oid::zero(ObjectFormat::Sha1),
        };
        let files = specs
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let old_blob = blob(&s.old);
                let new_blob = blob(&s.new);
                let status = match (&s.old, &s.new, &s.old_path) {
                    (None, _, _) => FileStatus::Added,
                    (_, None, _) => FileStatus::Deleted,
                    (_, _, Some(_)) => FileStatus::Renamed,
                    _ => FileStatus::Modified,
                };
                let mode = Some(Mode(0o100644));
                let path = |p: &str| Some(GitPath::from_bytes(p.as_bytes()));
                FileChange {
                    idx: idx as u32,
                    status,
                    old_path: s
                        .old
                        .as_ref()
                        .and(path(s.old_path.as_deref().unwrap_or(&s.path))),
                    new_path: s.new.as_ref().and(path(&s.path)),
                    old_mode: s.old.as_ref().and(mode),
                    new_mode: s.new.as_ref().and(mode),
                    old_blob,
                    new_blob,
                    similarity: s.old_path.as_ref().map(|_| 90),
                    kind: s.kind,
                    generated: false,
                }
            })
            .collect();
        Arc::new(MemProvider {
            files: Arc::new(files),
            blobs,
            broken: Mutex::new(HashSet::new()),
            loads: AtomicUsize::new(0),
        })
    }

    pub fn load_count(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    /// Makes loading file `idx`'s blobs fail (like objects missing from a
    /// shallow clone), or succeed again.
    pub fn set_broken(&self, idx: usize, broken: bool) {
        let change = &self.files[idx];
        let mut set = self.broken.lock().unwrap();
        for oid in [&change.old_blob, &change.new_blob] {
            if oid.is_zero() {
                continue;
            }
            if broken {
                set.insert(oid.as_str().to_owned());
            } else {
                set.remove(oid.as_str());
            }
        }
    }

    fn check(&self, oid: &Oid) -> anyhow::Result<()> {
        if self.broken.lock().unwrap().contains(oid.as_str()) {
            anyhow::bail!("object {} is missing", oid.short());
        }
        Ok(())
    }
}

impl DiffProvider for MemProvider {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }

    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }

    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.check(oid)?;
        self.blobs
            .get(oid.as_str())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no blob {oid:?}"))
    }

    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        self.check(oid)?;
        self.blobs
            .get(oid.as_str())
            .map(|b| b.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("no blob {oid:?}"))
    }
}

/// `n` lines `"<prefix> <i>"`, each ending in `\n`.
pub fn numbered(prefix: &str, n: u32) -> Vec<String> {
    (0..n).map(|i| format!("{prefix} {i}\n")).collect()
}

/// Options with the test font size, Pierre Light and the given layout.
pub fn options(layout: LayoutMode) -> ViewportOptions {
    ViewportOptions {
        layout,
        code_font_size: FONT_SIZE,
        theme: Arc::new(ViewportTheme::from_zed(pierre_theme(Appearance::Light))),
        ..ViewportOptions::default()
    }
}

/// Opens a window of `width × height` showing a viewport over `provider`,
/// and lets every background task finish.
pub fn open(
    cx: &mut TestAppContext,
    provider: Arc<MemProvider>,
    opts: ViewportOptions,
    width: f32,
    height: f32,
) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    assert_sandboxed();
    let window = cx.open_window(size(px(width), px(height)), move |window, cx| {
        DiffViewport::new(provider as Arc<dyn DiffProvider>, opts, window, cx)
    });
    let view = window.root(cx).expect("window has a root view");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    settle(cx);
    (view, cx)
}

/// Runs background work and redraws until nothing is left to do.
pub fn settle(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
    }
    cx.run_until_parked();
}

/// Draws one frame without running background work (loads and highlights
/// started by it stay pending).
pub fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
}

pub fn debug(view: &Entity<DiffViewport>, cx: &mut VisualTestContext) -> ViewportDebug {
    view.read_with(cx, |v, _| v.debug())
}

/// Scrolls the viewport by `dy` pixels (positive = down) with a wheel event
/// in the middle of the window.
pub fn wheel(cx: &mut VisualTestContext, dy: f32) {
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(200.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-dy))),
        modifiers: Modifiers::default(),
        ..Default::default()
    });
    settle(cx);
}

pub fn set_options(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
    f: impl FnOnce(&mut ViewportOptions),
) {
    let mut opts = view.read_with(cx, |v, _| v.options().clone());
    f(&mut opts);
    view.update(cx, |v, cx| v.set_options(opts, cx));
    settle(cx);
}

/// Every viewport event, recorded while the returned subscription lives.
pub fn record_events(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
) -> (Rc<RefCell<Vec<ViewportEvent>>>, Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let view = view.clone();
    let sub = cx.update(move |_, cx| {
        cx.subscribe(&view, move |_, e: &ViewportEvent, _| {
            sink.borrow_mut().push(e.clone())
        })
    });
    (events, sub)
}

/// The stats of the last frame among recorded `events`.
pub fn last_stats(events: &Rc<RefCell<Vec<ViewportEvent>>>) -> FrameStats {
    events
        .borrow()
        .iter()
        .rev()
        .find_map(|e| match e {
            ViewportEvent::FrameStats(s) => Some(*s),
            _ => None,
        })
        .expect("a frame was painted")
}

/// Every recorded frame's stats, in order.
pub fn all_stats(events: &Rc<RefCell<Vec<ViewportEvent>>>) -> Vec<FrameStats> {
    events
        .borrow()
        .iter()
        .filter_map(|e| match e {
            ViewportEvent::FrameStats(s) => Some(*s),
            _ => None,
        })
        .collect()
}

/// Text painted within visible row `i` of `d` as `(x, text)`, left to right
/// (x relative to the viewport's left edge).
pub fn texts_in_row(d: &ViewportDebug, i: usize) -> Vec<(f32, String)> {
    let (top, height) = d.row_bounds[i];
    let mut texts: Vec<(f32, String)> = d
        .painted_text
        .iter()
        .filter(|(_, y, _)| *y >= top && *y < top + height)
        .map(|(x, _, t)| (*x, t.clone()))
        .collect();
    texts.sort_by(|a, b| a.0.total_cmp(&b.0));
    texts
}

/// Asserts that `actual` (from [`texts_in_row`]) is `expected`, x within
/// 0.01 px.
pub fn assert_texts(actual: &[(f32, String)], expected: &[(f32, &str)]) {
    let same = actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a.0 - e.0).abs() < 0.01 && a.1 == e.1);
    assert!(same, "painted {actual:?}, expected {expected:?}");
}

/// Solid quads of the last frame as `(x, y, width, height, color)` in
/// window pixels (unscaled).
pub fn quads(cx: &mut VisualTestContext) -> Vec<(f32, f32, f32, f32, Hsla)> {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        window
            .painted_quads()
            .into_iter()
            .filter_map(|q| {
                let color = q.background.as_solid()?;
                let b = q.bounds;
                Some((
                    b.origin.x.0 / scale,
                    b.origin.y.0 / scale,
                    b.size.width.0 / scale,
                    b.size.height.0 / scale,
                    color,
                ))
            })
            .collect()
    })
}

/// Quads of exactly `color`.
pub fn quads_of(cx: &mut VisualTestContext, color: Hsla) -> Vec<(f32, f32, f32, f32)> {
    quads(cx)
        .into_iter()
        .filter(|q| q.4 == color)
        .map(|(x, y, w, h, _)| (x, y, w, h))
        .collect()
}

pub fn px_f32(p: Pixels) -> f32 {
    p.as_f32()
}

/// A unified row as `ViewportDebug::visible_rows` prints it (1-based numbers).
pub fn unified(old: Option<u32>, new: Option<u32>, marker: char, text: &str) -> String {
    let n = |v: Option<u32>| v.map_or(String::new(), |v| v.to_string());
    format!("{:>5} {:>5} {} {}", n(old), n(new), marker, text)
}

/// A split row as `ViewportDebug::visible_rows` prints it: `None` sides are
/// empty cells.
pub fn split(left: Option<(u32, char, &str)>, right: Option<(u32, char, &str)>) -> String {
    let cell = |c: Option<(u32, char, &str)>| match c {
        Some((n, m, t)) => format!("{n:>5} {m} {t}"),
        None => format!("{:>5} {} {}", "", ' ', ""),
    };
    format!("{} │ {}", cell(left), cell(right))
}
