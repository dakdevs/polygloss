//! The viewport's colors from a Zed theme (design §11.10, plan T6.2): the
//! `polygloss.*` keys, their fallbacks for themes without them, and the code
//! font's system-mono aliases and fallback.

use std::borrow::Cow;
use std::sync::Arc;

use gpui_kit::{
    Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, Hsla, LineLayout,
    NoopTextSystem, Pixels, PlatformTextSystem, RenderGlyphParams, Rgba as GpuiRgba, Size,
    TestAppContext, TestDispatcher, TextRenderingMode,
};
use polygloss_highlight::{
    Appearance, Rgba, ZedTheme, default_theme, load_theme_family, pierre_theme,
};
use polygloss_viewport::kit::{SYSTEM_MONO_ALIASES, SYSTEM_MONO_FONT, kit_fonts};
use polygloss_viewport::{LayoutMode, ViewportOptions, ViewportTheme};

use crate::support::{MemProvider, Spec, open, options, sandbox};

fn color(hex: &str) -> Hsla {
    gpui_kit::rgba(Rgba::parse(hex).expect("a color").to_u32()).into()
}

fn rgb(c: Hsla) -> GpuiRgba {
    GpuiRgba::from(c)
}

/// Channel distance between two colors (RGB only).
fn distance(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (rgb(a), rgb(b));
    (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs()
}

/// A theme with only the given style keys.
fn theme(appearance: &str, style: serde_json::Value) -> ZedTheme {
    let json = serde_json::json!({
        "name": "Test",
        "themes": [{ "name": "Test", "appearance": appearance, "style": style }],
    });
    load_theme_family(&json.to_string())
        .unwrap()
        .themes
        .remove(0)
}

#[test]
fn viewport_theme_reads_polygloss_keys() {
    // Polygloss Light (docs/research/redesign-reference.md, by hand).
    let t = ViewportTheme::from_zed(default_theme(Appearance::Light));
    for (field, hex) in [
        (t.canvas, "#f8f8f6"),
        (t.card_background, "#ffffff"),
        (t.card_border, "#e7e7e7"),
        (t.pill_background, "#f2f2f1"),
        (t.added_line_number, "#3f9a45"),
        (t.removed_line_number, "#c4433c"),
        (t.added_gutter, "#d9f2e0"),
        (t.removed_gutter, "#fbe1df"),
        (t.stat_added, "#3c7849"),
        (t.stat_removed, "#aa3c36"),
        (t.commit_sha, "#a8621f"),
    ] {
        assert_eq!(field, color(hex), "{hex}");
    }
    let dark = ViewportTheme::from_zed(default_theme(Appearance::Dark));
    assert_eq!(dark.canvas, color("#111113"));
    assert_eq!(dark.card_background, color("#18181b"));
    assert_eq!(dark.added_gutter, color("#1f3524"));
    assert_eq!(dark.commit_sha, color("#e3a25b"));

    // A `polygloss.*` key wins over the standard keys it falls back to:
    // Pierre Light with every one of them set to a color of its own.
    let mut pierre = pierre_theme(Appearance::Light).clone();
    for (key, hex) in [
        ("polygloss.created.line_number", "#010101"),
        ("polygloss.deleted.line_number", "#020202"),
        ("polygloss.created.gutter_background", "#030303"),
        ("polygloss.deleted.gutter_background", "#040404"),
        ("polygloss.stat.added", "#050505"),
        ("polygloss.stat.deleted", "#060606"),
        ("polygloss.commit_sha", "#070707"),
    ] {
        pierre.style.insert(key.into(), hex.into());
    }
    let t = ViewportTheme::from_zed(&pierre);
    assert_eq!(
        [
            t.added_line_number,
            t.removed_line_number,
            t.added_gutter,
            t.removed_gutter,
            t.stat_added,
            t.stat_removed,
            t.commit_sha,
        ],
        [
            color("#010101"),
            color("#020202"),
            color("#030303"),
            color("#040404"),
            color("#050505"),
            color("#060606"),
            color("#070707"),
        ]
    );
}

#[test]
fn viewport_theme_derives_polygloss_keys_for_pierre_and_a_minimal_theme() {
    // Pierre Light has no `polygloss.*` key: each comes from its standard
    // fallback (assets/themes/pierre-light.json values).
    let t = ViewportTheme::pierre(Appearance::Light);
    assert_eq!(t.canvas, color("#f5f5f5")); // background
    assert_eq!(t.card_background, color("#ffffff")); // editor.background
    assert_eq!(t.card_border, color("#e5e5e5")); // border
    assert_eq!(t.pill_background, color("#ededed")); // element.background
    assert_eq!(t.added_line_number, color("#18a46c")); // created
    assert_eq!(t.removed_line_number, color("#d52c36")); // deleted
    assert_eq!(t.stat_added, color("#18a46c")); // version_control.added
    assert_eq!(t.stat_removed, color("#d52c36")); // version_control.deleted
    assert_eq!(t.commit_sha, color("#a631be")); // syntax type.builtin
    // The gutters: the translucent row tint (`#18a46c33`), slightly stronger.
    for (gutter, row) in [
        (t.added_gutter, color("#18a46c33")),
        (t.removed_gutter, color("#d52c3633")),
    ] {
        assert!(
            distance(gutter, row) < 0.01,
            "{gutter:?} keeps {row:?}'s hue"
        );
        assert!(gutter.a > row.a && gutter.a < 0.5, "{gutter:?} vs {row:?}");
    }

    // A minimal theme: opaque row tints, no `type.builtin`, no stat colors.
    let t = ViewportTheme::from_zed(&theme(
        "light",
        serde_json::json!({
            "background": "#fafafa",
            "editor.background": "#ffffff",
            "created": "#00aa00",
            "created.background": "#e0ffe0",
            "deleted": "#cc0000",
            "deleted.background": "#ffe0e0",
            "terminal.ansi.yellow": "#aa8800",
        }),
    ));
    assert_eq!(t.canvas, color("#fafafa"));
    assert_eq!(t.card_background, color("#ffffff"));
    assert_eq!(t.added_line_number, color("#00aa00"));
    assert_eq!(t.stat_added, color("#00aa00"));
    assert_eq!(t.stat_removed, color("#cc0000"));
    assert_eq!(t.commit_sha, color("#aa8800")); // terminal.ansi.yellow
    // An opaque tint gets a little of the accent: still a tint, not the bar.
    for (gutter, row, accent) in [
        (t.added_gutter, color("#e0ffe0"), color("#00aa00")),
        (t.removed_gutter, color("#ffe0e0"), color("#cc0000")),
    ] {
        assert_eq!(gutter.a, 1.0);
        assert!(
            distance(gutter, row) > 0.02,
            "{gutter:?} differs from {row:?}"
        );
        assert!(
            distance(gutter, accent) < distance(row, accent),
            "{gutter:?} leans to {accent:?}"
        );
        assert!(
            distance(gutter, row) < distance(gutter, accent),
            "{gutter:?} stays near {row:?}"
        );
    }

    // No colors at all still resolves to visible ones.
    let empty = ViewportTheme::from_zed(&theme("dark", serde_json::json!({})));
    for c in [
        empty.canvas,
        empty.card_border,
        empty.commit_sha,
        empty.added_gutter,
    ] {
        assert!(c.a > 0.0, "{c:?}");
    }
}

/// GPUI's no-op text system, knowing only `families`: any other family
/// fails to load, as on a Mac without it.
struct FamiliesTextSystem {
    inner: NoopTextSystem,
    families: Vec<&'static str>,
}

impl PlatformTextSystem for FamiliesTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> anyhow::Result<()> {
        self.inner.add_fonts(fonts)
    }
    fn all_font_names(&self) -> Vec<String> {
        self.families.iter().map(|f| (*f).to_owned()).collect()
    }
    fn font_id(&self, descriptor: &Font) -> anyhow::Result<FontId> {
        self.families
            .iter()
            .position(|f| *f == descriptor.family.as_ref())
            .map(FontId)
            .ok_or_else(|| anyhow::anyhow!("no font {:?}", descriptor.family))
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

/// The family a viewport draws code in when `code_font` is configured, on a
/// Mac with `families` installed (Helvetica: GPUI's last-resort fallback).
fn drawn_family(code_font: &'static str, families: &[&'static str]) -> String {
    let text_system = FamiliesTextSystem {
        inner: NoopTextSystem,
        families: [families, &["Helvetica"]].concat(),
    };
    let mut cx =
        TestAppContext::build_with_text_system(TestDispatcher::new(0), None, Arc::new(text_system));
    let provider = MemProvider::new(vec![Spec::modified("a.rs", "a\n", "b\n")]);
    let opts = ViewportOptions {
        code_font: code_font.into(),
        ..options(LayoutMode::Unified)
    };
    let (view, cx) = open(&mut cx, provider, opts, 800., 400.);
    view.read_with(cx, |v, _| v.code_font().family.to_string())
}

#[test]
fn sf_mono_alias_resolves_to_the_system_mono_family() {
    let _sb = sandbox();
    assert_eq!(SYSTEM_MONO_ALIASES, ["SF Mono", "System Mono"]);
    for alias in SYSTEM_MONO_ALIASES {
        // Even where Apple's developer fonts install a family by that name.
        assert_eq!(
            drawn_family(alias, &[SYSTEM_MONO_FONT, "SF Mono", "System Mono"]),
            SYSTEM_MONO_FONT,
            "{alias}"
        );
        // gpui-kit's monospace family too, installed or not.
        for installed in [true, false] {
            assert_eq!(
                kit_fonts(alias, installed).mono,
                SYSTEM_MONO_FONT,
                "{alias}"
            );
        }
    }
    // Other families are themselves.
    assert_eq!(drawn_family("Lilex", &[SYSTEM_MONO_FONT, "Lilex"]), "Lilex");
}

#[test]
fn missing_code_font_falls_back_to_system_mono() {
    let _sb = sandbox();
    // Menlo is installed, but the fallback is the system mono, not Menlo.
    assert_eq!(
        drawn_family("No Such Font", &[SYSTEM_MONO_FONT, "Menlo"]),
        SYSTEM_MONO_FONT
    );
}
