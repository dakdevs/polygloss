//! The screenshot baseline runner (clean-room; design §20, ADR-0017).
//!
//! **Rendering.** [`headless_app`] is GPUI's `HeadlessAppContext`: the test
//! platform (deterministic scheduler, windows at a fixed 2× scale, no
//! `NSWindow`) with the real macOS text system (CoreText) and GPUI's Metal
//! headless renderer, so text is shaped and rasterized as in the app and the
//! capture is the frame's scene rendered to a texture. [`open_window`] opens a
//! 1280×800 window, so a capture is 2560×1600. Only the main thread can create
//! the text system, which is why the `e2e` binary runs on
//! [`super::harness`]. `VisualTestAppContext` (what the design first named)
//! opens real off-screen `NSWindow`s whose scale follows the display, so it
//! cannot pin 2×.
//!
//! **Comparing.** [`assert_screenshot`] compares a capture with
//! `tests/baselines/<name>.png`, where `<name>` is the running test's name
//! with `_` → `-` (`e2e_viewport_split_pierre_light` →
//! `e2e-viewport-split-pierre-light.png`). A pixel differs when any channel
//! differs by more than [`CHANNEL_TOLERANCE`]; at most
//! [`MAX_DIFFERING_PER_MILLE`]‰ of the pixels may differ. On failure it writes
//! `<name>.actual.png` and (same size only) `<name>.diff.png` next to the
//! baseline (both gitignored): differing pixels in [`DIFF_MARK`], the rest a
//! faded copy of the baseline. `UPDATE_BASELINE=1` writes the capture as the
//! new baseline instead. Baselines depend on the fonts and the OS's text
//! rendering, so they are recorded on the macOS arm64 runner image the plan
//! pins; re-record them whenever what the viewport paints changes on purpose.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, Entity, HeadlessAppContext, Modifiers, MouseMoveEvent, PlatformInput,
    Render, Window, WindowHandle, point, px, size,
};
use image::{Rgba, RgbaImage};

/// Window size in points; captures are twice that in pixels.
pub const WINDOW_WIDTH: f32 = 1280.0;
pub const WINDOW_HEIGHT: f32 = 800.0;
/// The test platform's fixed scale factor.
pub const SCALE: u32 = 2;
/// Largest per-channel difference that still counts as the same pixel.
pub const CHANNEL_TOLERANCE: u8 = 2;
/// At most this many pixels per thousand may differ (0.1%).
pub const MAX_DIFFERING_PER_MILLE: u64 = 1;
/// Differing pixels in a `.diff.png`.
pub const DIFF_MARK: Rgba<u8> = Rgba([255, 0, 64, 255]);

/// GPUI with real text shaping and Metal rendering on the test platform,
/// with the bundled Lilex registered. Must run on the main thread (see the
/// module docs).
pub fn headless_app() -> HeadlessAppContext {
    headless_app_with_assets(Arc::new(()))
}

/// [`headless_app`] with an asset source behind the app's own icons
/// (`polygloss_app::assets::AppIcons`): given gpui-kit's icons
/// (`Arc::new(gpui_kit::assets::Assets)`), the app's `AppAssets`. Every
/// capture runs with Reduce Motion on, so nothing is caught mid-animation.
pub fn headless_app_with_assets(assets: Arc<dyn gpui_kit::AssetSource>) -> HeadlessAppContext {
    assert_eq!(
        std::thread::current().name(),
        Some("main"),
        "screenshots need the main thread: run them in the `e2e` binary"
    );
    let mut cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(WithAppIcons(assets)),
        gpui_kit::platform::current_headless_renderer,
    );
    // The app's bundled code font (T3.3): screenshots draw Lilex, as the
    // app does, not the system's Menlo fallback.
    cx.update(polygloss_app::theme::fonts::register_fonts);
    cx.update(|cx| cx.set_reduce_motion(true));
    cx
}

/// `polygloss_app::assets::AppIcons` first, then another source.
struct WithAppIcons(Arc<dyn gpui_kit::AssetSource>);

impl gpui_kit::AssetSource for WithAppIcons {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match polygloss_app::assets::AppIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => self.0.load(path),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<gpui_kit::SharedString>> {
        let mut paths = self.0.list(path)?;
        paths.extend(polygloss_app::assets::AppIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Where [`open_window`] parks the pointer: outside the window.
pub const POINTER_AWAY: (f32, f32) = (-100.0, -100.0);

/// Opens a 1280×800 window whose root view `build` returns, with the
/// pointer outside it. The test platform starts the pointer at the window's
/// origin, where it would hover whatever is there (the first file header's
/// collapse chevron) in every capture; a real window only sees the pointer
/// once the user moves it in.
pub fn open_window<V: Render + 'static>(
    cx: &mut HeadlessAppContext,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> WindowHandle<V> {
    let window = cx
        .open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), build)
        .expect("open a headless window");
    park_pointer(cx, *window);
    window
}

/// Moves the pointer outside `window` (see [`POINTER_AWAY`]).
pub fn park_pointer(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: point(px(POINTER_AWAY.0), px(POINTER_AWAY.1)),
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        assert_eq!(
            window.mouse_position(),
            point(px(POINTER_AWAY.0), px(POINTER_AWAY.1)),
            "the pointer is parked outside the window"
        );
    })
    .expect("window is open");
}

/// Runs every pending task, then draws one frame.
pub fn draw(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("window is open");
}

/// The last drawn frame, rendered by Metal (2560×1600).
pub fn capture(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> RgbaImage {
    let image = cx
        .capture_screenshot(window)
        .expect("render the frame with Metal");
    assert_eq!(
        image.dimensions(),
        (WINDOW_WIDTH as u32 * SCALE, WINDOW_HEIGHT as u32 * SCALE),
        "captures are 1280×800 @2x"
    );
    image
}

/// `tests/baselines/` of this crate.
pub fn baselines_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/baselines")
}

/// The baseline file stem of a test: its name with `_` → `-`.
pub fn baseline_name(test_name: &str) -> String {
    test_name.replace('_', "-")
}

/// A pixel-by-pixel comparison of two same-size images.
#[derive(Debug, Clone)]
pub struct Comparison {
    /// Pixels with a channel more than [`CHANNEL_TOLERANCE`] apart.
    pub differing: u64,
    pub total: u64,
    /// Differing pixels in [`DIFF_MARK`], the rest a faded expected image.
    pub diff: RgbaImage,
}

impl Comparison {
    /// At most [`MAX_DIFFERING_PER_MILLE`]‰ of the pixels differ.
    pub fn within_budget(&self) -> bool {
        self.differing * 1000 <= self.total * MAX_DIFFERING_PER_MILLE
    }
}

/// Compares `actual` with `expected`; `None` when their sizes differ.
pub fn compare(actual: &RgbaImage, expected: &RgbaImage) -> Option<Comparison> {
    if actual.dimensions() != expected.dimensions() {
        return None;
    }
    let mut diff = RgbaImage::new(expected.width(), expected.height());
    let mut differing = 0;
    for ((a, e), d) in actual
        .pixels()
        .zip(expected.pixels())
        .zip(diff.pixels_mut())
    {
        let differs =
            a.0.iter()
                .zip(e.0)
                .any(|(a, e)| a.abs_diff(e) > CHANNEL_TOLERANCE);
        *d = if differs {
            differing += 1;
            DIFF_MARK
        } else {
            // A faded copy for orientation: 25% of the pixel over white.
            let fade = |c: u8| 255 - (255 - c) / 4;
            Rgba([fade(e[0]), fade(e[1]), fade(e[2]), 255])
        };
    }
    Some(Comparison {
        differing,
        total: u64::from(expected.width()) * u64::from(expected.height()),
        diff,
    })
}

/// What [`check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Matched {
        differing: u64,
    },
    /// `update` was set: the capture is the new baseline.
    Updated,
    /// No baseline yet.
    Missing,
    SizeMismatch {
        actual: (u32, u32),
        expected: (u32, u32),
    },
    Mismatch {
        differing: u64,
        total: u64,
    },
}

impl Verdict {
    pub fn passed(&self) -> bool {
        matches!(self, Verdict::Matched { .. } | Verdict::Updated)
    }
}

/// Compares `actual` with `<dir>/<baseline_name(test_name)>.png`, or writes
/// it there when `update`. Writes `.actual.png` (and `.diff.png` for a
/// same-size mismatch) on failure and removes stale ones on success.
pub fn check(
    dir: &Path,
    test_name: &str,
    actual: &RgbaImage,
    update: bool,
) -> std::io::Result<Verdict> {
    let stem = baseline_name(test_name);
    let baseline = dir.join(format!("{stem}.png"));
    let actual_path = dir.join(format!("{stem}.actual.png"));
    let diff_path = dir.join(format!("{stem}.diff.png"));
    let save = |image: &RgbaImage, path: &Path| {
        image
            .save(path)
            .map_err(|e| std::io::Error::other(format!("write {}: {e}", path.display())))
    };
    let remove_stale = || -> std::io::Result<()> {
        for path in [&actual_path, &diff_path] {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    };

    if update {
        save(actual, &baseline)?;
        remove_stale()?;
        return Ok(Verdict::Updated);
    }
    if !baseline.exists() {
        remove_stale()?;
        save(actual, &actual_path)?;
        return Ok(Verdict::Missing);
    }
    let expected = image::open(&baseline)
        .map_err(|e| std::io::Error::other(format!("read {}: {e}", baseline.display())))?
        .to_rgba8();
    remove_stale()?;
    let Some(cmp) = compare(actual, &expected) else {
        save(actual, &actual_path)?;
        return Ok(Verdict::SizeMismatch {
            actual: actual.dimensions(),
            expected: expected.dimensions(),
        });
    };
    if cmp.within_budget() {
        return Ok(Verdict::Matched {
            differing: cmp.differing,
        });
    }
    save(actual, &actual_path)?;
    save(&cmp.diff, &diff_path)?;
    Ok(Verdict::Mismatch {
        differing: cmp.differing,
        total: cmp.total,
    })
}

/// Checks `actual` against the running E2E test's baseline (see the module
/// docs); `UPDATE_BASELINE=1` rewrites it. Panics on a mismatch.
pub fn assert_screenshot(actual: &RgbaImage) {
    let test = super::harness::current_test().expect("assert_screenshot runs inside an E2E test");
    let update = std::env::var_os("UPDATE_BASELINE").is_some_and(|v| v == "1");
    let dir = baselines_dir();
    std::fs::create_dir_all(&dir).expect("create tests/baselines");
    let verdict = check(&dir, test, actual, update).expect("read or write screenshot files");
    let stem = dir.join(baseline_name(test));
    match verdict {
        Verdict::Matched { .. } => {}
        Verdict::Updated => eprintln!("{test}: wrote {}.png", stem.display()),
        Verdict::Missing => panic!(
            "{test}: no baseline {}.png; the capture is in {0}.actual.png. \
             Look at it, then record it with UPDATE_BASELINE=1.",
            stem.display()
        ),
        Verdict::SizeMismatch { actual, expected } => panic!(
            "{test}: capture is {actual:?} px, baseline {expected:?} px; see {}.actual.png",
            stem.display()
        ),
        Verdict::Mismatch { differing, total } => panic!(
            "{test}: {differing} of {total} pixels differ (more than {MAX_DIFFERING_PER_MILLE}‰); \
             see {0}.actual.png and {0}.diff.png, or rerun with UPDATE_BASELINE=1 \
             if the change is intended",
            stem.display()
        ),
    }
}
