//! gpui-kit initialization without its startup font scan (plan T2.10.2).
//!
//! `gpui_kit::init` lists every installed font family (a CoreText scan of
//! every face, ≈ 440 ms on the dev machine, before the first window) unless
//! its theme names both of its families explicitly. These tests count the
//! scans through a platform text system that records `all_font_names` calls.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::{
    Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, Hsla, LineLayout,
    NoopTextSystem, Pixels, PlatformTextSystem, RenderGlyphParams, Size, TestAppContext,
    TestDispatcher, TextRenderingMode,
};
use polygloss_viewport::kit::{KitFonts, SYSTEM_MONO_FONT, SYSTEM_UI_FONT, init_kit, kit_fonts};

use crate::support::sandbox;

/// GPUI's no-op text system, counting how often the installed fonts are
/// listed.
struct CountingTextSystem {
    inner: NoopTextSystem,
    listed: Arc<AtomicUsize>,
}

impl PlatformTextSystem for CountingTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> anyhow::Result<()> {
        self.inner.add_fonts(fonts)
    }
    fn all_font_names(&self) -> Vec<String> {
        self.listed.fetch_add(1, Ordering::SeqCst);
        // What a Mac has, as far as gpui-kit's probes look.
        vec!["Menlo".into(), "Monaco".into()]
    }
    fn font_id(&self, descriptor: &Font) -> anyhow::Result<FontId> {
        self.inner.font_id(descriptor)
    }
    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.inner.font_metrics(font_id)
    }
    fn typographic_bounds(
        &self,
        font_id: FontId,
        glyph_id: GlyphId,
    ) -> anyhow::Result<Bounds<f32>> {
        self.inner.typographic_bounds(font_id, glyph_id)
    }
    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> anyhow::Result<Size<f32>> {
        self.inner.advance(font_id, glyph_id)
    }
    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.inner.glyph_for_char(font_id, ch)
    }
    fn glyph_raster_bounds(
        &self,
        params: &RenderGlyphParams,
    ) -> anyhow::Result<Bounds<DevicePixels>> {
        self.inner.glyph_raster_bounds(params)
    }
    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> anyhow::Result<(Size<DevicePixels>, Vec<u8>)> {
        self.inner.rasterize_glyph(params, raster_bounds)
    }
    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.inner.layout_line(text, font_size, runs)
    }
    fn recommended_rendering_mode(&self, font_id: FontId, font_size: Pixels) -> TextRenderingMode {
        self.inner.recommended_rendering_mode(font_id, font_size)
    }
    fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        self.inner.glyph_dilation_for_color(color)
    }
}

/// A test app whose text system counts font listings into the returned
/// counter.
fn counting_app() -> (TestAppContext, Arc<AtomicUsize>) {
    let listed = Arc::new(AtomicUsize::new(0));
    let text_system = CountingTextSystem {
        inner: NoopTextSystem,
        listed: listed.clone(),
    };
    let cx =
        TestAppContext::build_with_text_system(TestDispatcher::new(0), None, Arc::new(text_system));
    (cx, listed)
}

/// gpui-kit's theme as JSON, fonts left out.
fn theme_without_fonts(cx: &TestAppContext) -> serde_json::Value {
    cx.update(|cx| {
        let mut value = serde_json::to_value(Theme::global(cx)).unwrap();
        let map = value.as_object_mut().unwrap();
        map.remove("font_family").unwrap();
        map.remove("mono_font_family").unwrap();
        value
    })
}

#[test]
fn init_kit_never_lists_installed_fonts() {
    let _sb = sandbox();
    let (cx, listed) = counting_app();
    cx.update(|cx| init_kit("Lilex", cx));
    assert_eq!(
        listed.load(Ordering::SeqCst),
        0,
        "init_kit listed the fonts"
    );
    cx.update(|cx| {
        let theme = Theme::global(cx);
        assert_eq!(theme.font_family.as_ref(), SYSTEM_UI_FONT);
        assert_ne!(theme.mono_font_family.as_ref(), "Menlo");
        assert_eq!(theme.mode, ThemeMode::Light);
    });
}

#[test]
fn plain_kit_init_lists_installed_fonts() {
    // Why `init_kit` exists: if gpui-kit stops scanning, this fails and the
    // helper can go.
    let _sb = sandbox();
    let (cx, listed) = counting_app();
    cx.update(gpui_kit::init);
    assert!(listed.load(Ordering::SeqCst) >= 1);
}

#[test]
fn init_kit_matches_plain_init_except_fonts() {
    let _sb = sandbox();
    let (ours, _) = counting_app();
    ours.update(|cx| init_kit("Lilex", cx));
    let (plain, _) = counting_app();
    plain.update(gpui_kit::init);
    assert_eq!(theme_without_fonts(&ours), theme_without_fonts(&plain));
}

#[test]
fn kit_fonts_name_the_code_font_or_the_system_mono() {
    let named = |ui: &str, mono: &str| KitFonts {
        ui: ui.to_owned().into(),
        mono: mono.to_owned().into(),
    };
    assert_eq!(kit_fonts("Lilex", true), named(SYSTEM_UI_FONT, "Lilex"));
    assert_eq!(
        kit_fonts("Lilex", false),
        named(SYSTEM_UI_FONT, SYSTEM_MONO_FONT)
    );
    // gpui-kit's own default mono family is probed (and scanned for) whenever
    // it is named, so it is never named.
    assert_eq!(
        kit_fonts("Menlo", true),
        named(SYSTEM_UI_FONT, SYSTEM_MONO_FONT)
    );
}
