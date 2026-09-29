//! Shared helpers for the viewport's GPUI tests: a per-test sandbox, an
//! in-memory `DiffProvider`, a window with a `DiffViewport` in it, and readers
//! for what it painted.
//!
//! Every test starts with `let _sb = sandbox();` (plan "Test hygiene"; `open`
//! checks it). GPUI's test platform shapes text with a no-op text system:
//! every char is `0.6 × font size` wide and the font is irrelevant, so
//! geometry is exact.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    Modifiers, MouseButton, ParentElement as _, Pixels, Render, ScrollDelta, ScrollWheelEvent,
    Styled as _, Subscription, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, Mode, ObjectFormat, Oid};
use polygloss_highlight::{Appearance, pierre_theme};
use polygloss_viewport::{
    ControlAction, DiffProvider, DiffViewport, FrameStats, LayoutMode, PipelineStats, ScrollTarget,
    ViewportDebug, ViewportEvent, ViewportOptions, ViewportTheme,
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
    /// Listed as generated (drawn collapsed, never loaded; design §12.3).
    pub generated: bool,
}

impl Spec {
    pub fn modified(path: &str, old: &str, new: &str) -> Spec {
        Spec {
            path: path.to_owned(),
            old: Some(old.to_owned()),
            new: Some(new.to_owned()),
            kind: FileKind::Text,
            old_path: None,
            generated: false,
        }
    }

    /// A modified text file listed as generated.
    pub fn generated(path: &str, old: &str, new: &str) -> Spec {
        Spec {
            generated: true,
            ..Spec::modified(path, old, new)
        }
    }

    pub fn added(path: &str, new: &str) -> Spec {
        Spec {
            path: path.to_owned(),
            old: None,
            new: Some(new.to_owned()),
            kind: FileKind::Text,
            old_path: None,
            generated: false,
        }
    }

    pub fn binary(path: &str) -> Spec {
        Spec {
            path: path.to_owned(),
            old: Some("\0old".to_owned()),
            new: Some("\0new".to_owned()),
            kind: FileKind::Binary,
            old_path: None,
            generated: false,
        }
    }
}

/// What [`MemProvider::mark`] logs.
pub const MARK: usize = usize::MAX;

/// An in-memory `DiffProvider` that counts blob loads, records their order
/// and can fail them.
pub struct MemProvider {
    files: Arc<Vec<FileChange>>,
    blobs: HashMap<String, Arc<[u8]>>,
    /// The file each blob belongs to.
    owners: HashMap<String, usize>,
    /// Blobs that fail to load (see [`MemProvider::set_broken`]).
    broken: Mutex<HashSet<String>>,
    /// The file of every `load_blob` call, in call order.
    order: Mutex<Vec<usize>>,
    pub loads: AtomicUsize,
}

impl MemProvider {
    pub fn new(specs: Vec<Spec>) -> Arc<MemProvider> {
        MemProvider::new_with(specs, |_| {})
    }

    /// Like [`MemProvider::new`], then lets `edit` adjust the file list's
    /// metadata (generated flags, modes, submodule ids) before anything reads
    /// it. Blobs stay keyed by the ids `new` assigned.
    pub fn new_with(specs: Vec<Spec>, edit: impl FnOnce(&mut [FileChange])) -> Arc<MemProvider> {
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
        let mut owners = HashMap::new();
        let files = specs
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let old_blob = blob(&s.old);
                let new_blob = blob(&s.new);
                for oid in [&old_blob, &new_blob] {
                    owners.insert(oid.as_str().to_owned(), idx);
                }
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
                    generated: s.generated,
                }
            })
            .collect::<Vec<_>>();
        let mut files = files;
        edit(&mut files);
        Arc::new(MemProvider {
            files: Arc::new(files),
            blobs,
            owners,
            broken: Mutex::new(HashSet::new()),
            order: Mutex::new(Vec::new()),
            loads: AtomicUsize::new(0),
        })
    }

    pub fn load_count(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    /// The file whose blob each `load_blob` call read, in call order, with
    /// the [`MemProvider::mark`]s in between.
    pub fn load_order(&self) -> Vec<usize> {
        self.order.lock().unwrap().clone()
    }

    /// Puts [`MARK`] into the [`MemProvider::load_order`] log (e.g. when a
    /// frame is painted), to see which reads came before and after.
    pub fn mark(&self) {
        self.order.lock().unwrap().push(MARK);
    }

    /// `load_blob` calls that read a blob of file `idx`.
    pub fn loads_of(&self, idx: usize) -> usize {
        self.order
            .lock()
            .unwrap()
            .iter()
            .filter(|&&f| f == idx)
            .count()
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
        if let Some(&idx) = self.owners.get(oid.as_str()) {
            self.order.lock().unwrap().push(idx);
        }
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
    provider: Arc<dyn DiffProvider>,
    opts: ViewportOptions,
    width: f32,
    height: f32,
) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    assert_sandboxed();
    let window = cx.open_window(size(px(width), px(height)), move |window, cx| {
        DiffViewport::new(provider, opts, window, cx)
    });
    let view = window.root(cx).expect("window has a root view");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    settle(cx);
    (view, cx)
}

/// Opens a window of `width × height` showing a viewport over `provider`
/// without running any background work. GPUI draws the first frame when the
/// window opens, so the loads it starts stay pending.
pub fn open_idle(
    cx: &mut TestAppContext,
    provider: Arc<dyn DiffProvider>,
    opts: ViewportOptions,
    width: f32,
    height: f32,
) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    open_idle_at(cx, provider, opts, width, height, ScrollTarget::File(0))
}

/// [`open_idle`] with the viewport scrolled to `target` before its first
/// frame.
pub fn open_idle_at(
    cx: &mut TestAppContext,
    provider: Arc<dyn DiffProvider>,
    opts: ViewportOptions,
    width: f32,
    height: f32,
    target: ScrollTarget,
) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    assert_sandboxed();
    let window = cx.open_window(size(px(width), px(height)), move |window, cx| {
        let mut view = DiffViewport::new(provider, opts, window, cx);
        view.scroll_to(target, cx);
        view
    });
    let view = window.root(cx).expect("window has a root view");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    (view, cx)
}

/// The viewport's background pipeline counters.
pub fn pipeline_stats(view: &Entity<DiffViewport>, cx: &mut VisualTestContext) -> PipelineStats {
    view.read_with(cx, |v, _| v.pipeline_stats())
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

/// Initializes gpui-kit (theme, key bindings) the way a host does before
/// opening windows; the header's ⋯ menu is a gpui-kit `PopupMenu`.
pub fn init_kit(cx: &mut TestAppContext) {
    let code_font = ViewportOptions::default().code_font;
    cx.update(|cx| polygloss_viewport::kit::init_kit(&code_font, cx));
}

/// Height of [`Inset`]'s toolbar above the viewport.
pub const TOOLBAR_H: f32 = 40.0;
/// Width of [`Inset`]'s side panel right of the viewport.
pub const SIDE_W: f32 = 100.0;

/// A host with chrome around the viewport, like the app's review tab: a
/// focusable toolbar [`TOOLBAR_H`] px tall above it and a side panel
/// [`SIDE_W`] px wide right of it, each counting the left presses it gets.
pub struct Inset {
    pub viewport: Entity<DiffViewport>,
    pub toolbar_focus: FocusHandle,
    pub toolbar_presses: Rc<Cell<u32>>,
    pub side_presses: Rc<Cell<u32>>,
}

impl Render for Inset {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (toolbar, side) = (self.toolbar_presses.clone(), self.side_presses.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(TOOLBAR_H))
                    .w_full()
                    .flex_none()
                    .track_focus(&self.toolbar_focus)
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        toolbar.set(toolbar.get() + 1)
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(div().flex_1().h_full().child(self.viewport.clone()))
                    .child(
                        div()
                            .w(px(SIDE_W))
                            .h_full()
                            .flex_none()
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                side.set(side.get() + 1)
                            }),
                    ),
            )
    }
}

/// Opens a `width × height` window with an [`Inset`] host around a viewport
/// over `provider` (the viewport is `width - SIDE_W` × `height - TOOLBAR_H`,
/// its top-left corner at `(0, TOOLBAR_H)`), focuses the toolbar, and lets
/// every background task finish.
pub fn open_inset(
    cx: &mut TestAppContext,
    provider: Arc<MemProvider>,
    opts: ViewportOptions,
    width: f32,
    height: f32,
) -> (Entity<Inset>, Entity<DiffViewport>, &mut VisualTestContext) {
    assert_sandboxed();
    let window = cx.open_window(size(px(width), px(height)), move |window, cx| {
        let viewport =
            cx.new(|cx| DiffViewport::new(provider as Arc<dyn DiffProvider>, opts, window, cx));
        Inset {
            viewport,
            toolbar_focus: cx.focus_handle(),
            toolbar_presses: Rc::new(Cell::new(0)),
            side_presses: Rc::new(Cell::new(0)),
        }
    });
    let host = window.root(cx).expect("window has a root view");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    let (viewport, focus) =
        host.read_with(cx, |h, _| (h.viewport.clone(), h.toolbar_focus.clone()));
    cx.update(|window, cx| window.focus(&focus, cx));
    settle(cx);
    (host, viewport, cx)
}

/// Clicks at window coordinates `(x, y)` and lets the result settle.
pub fn click_window(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
    cx.simulate_click(point(px(x), px(y)), Modifiers::default());
    settle(cx);
}

/// Clicks at viewport-relative `(x, y)` (the viewport fills the window) and
/// lets the result settle.
pub fn click_at(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
    cx.simulate_click(point(px(x), px(y)), Modifiers::default());
    settle(cx);
}

/// Bounds `(x, y, width, height)` of the control doing `action` in the last
/// frame, relative to the viewport; panics when it was not painted.
pub fn control(d: &ViewportDebug, action: ControlAction) -> (f32, f32, f32, f32) {
    d.controls
        .iter()
        .find(|c| c.action == action)
        .map(|c| c.bounds)
        .unwrap_or_else(|| panic!("no control {action:?} in {:?}", d.controls))
}

/// Clicks the middle of the control doing `action`.
pub fn click_control(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
    action: ControlAction,
) {
    let (x, y, w, h) = control(&debug(view, cx), action);
    click_at(cx, x + w / 2.0, y + h / 2.0);
}

/// Clicks the open popup menu's item labeled `label` (gpui-kit test
/// locators: items are identified by index, named by their label).
pub fn click_menu_item(cx: &mut VisualTestContext, label: &str) {
    use gpui_kit::test::TestWindowExt as _;
    let ix = cx.update(|window, _| {
        let menu = window.within("popup-menu");
        (0usize..32)
            .find(|&ix| {
                menu.try_find(ix)
                    .is_some_and(|item| item.label() == Some(label))
            })
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
    });
    cx.update(|window, cx| window.within("popup-menu").click(ix, cx));
    settle(cx);
}
