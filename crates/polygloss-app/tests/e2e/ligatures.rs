//! The code font draws code as typed (T5.10): with the default settings
//! (`buffer_font.ligatures: false`) Lilex's programming ligatures are off, so
//! `->` shapes to the glyphs of `-` and `>` rather than an arrow. Checked
//! with the real macOS text system (CoreText), which is what applies the
//! OpenType features; GPUI's test text system ignores them.

use std::sync::Arc;

use gpui_kit::{Font, GlyphId, TextRun, Window, black, px, size};
use polygloss_app::settings::Settings;
use polygloss_app::theme::fonts::LILEX_FAMILY;
use polygloss_app::{startup, window};
use polygloss_core::review::Core;
use polygloss_viewport::code_font;
use polygloss_viewport::kit::is_installed;

use crate::support::Sandbox;
use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH};

pub const TESTS: &[Test] = &crate::tests![e2e_code_font_shapes_operators_as_typed];

/// The glyphs `text` shapes to in `font` on one line.
fn glyphs(window: &Window, text: &str, font: &Font) -> Vec<GlyphId> {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .layout_line(text, px(13.), &[run], None)
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| g.id))
        .collect()
}

fn e2e_code_font_shapes_operators_as_typed() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, _) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    cx.update_window(handle, |_, window, _| {
        assert!(
            is_installed(window.text_system(), LILEX_FAMILY),
            "the bundled Lilex is registered"
        );
        let settings = Settings::default().buffer_font;
        assert_eq!(settings.family, LILEX_FAMILY);
        let off = code_font(settings.family.clone(), settings.ligatures);
        let on = code_font(settings.family, true);
        for op in ["->", "=>", "!=", ">="] {
            // Each character on its own can never form a ligature.
            let typed: Vec<GlyphId> = op
                .chars()
                .flat_map(|c| glyphs(window, &c.to_string(), &off))
                .collect();
            assert_eq!(
                glyphs(window, op, &off),
                typed,
                "`{op}` is drawn as typed by default"
            );
            // The control: the font does have a ligature for it, so the
            // check above tests the setting, not a font without them.
            assert_ne!(
                glyphs(window, op, &on),
                typed,
                "Lilex has a `{op}` ligature when they are on"
            );
        }
    })
    .expect("the window is open");
}
