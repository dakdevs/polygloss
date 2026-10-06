//! How the palette and the cheat sheet show a key: gpui-kit's `Kbd`
//! spelling (`⌘K`, `⇧⏎`), except that Escape reads `Esc`. Its `⎋` glyph is
//! easy to mistake for a reload arrow.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    App, Div, InteractiveElement as _, Keystroke, ParentElement as _, Styled as _, div, px,
};

use crate::space::{TextStyleExt as _, height, pad, radius, text};

/// `stroke` as a key cap shows it: `⌘K`, `⇧⏎`, `Esc`, `⌘Esc`.
pub fn key_label(stroke: &Keystroke) -> String {
    if stroke.key == "escape" {
        let mut esc = stroke.clone();
        // Spelled out: `Kbd::format` capitalizes a key name it has no
        // symbol for.
        esc.key = "esc".into();
        Kbd::format(&esc)
    } else {
        Kbd::format(stroke)
    }
}

/// A key cap for `stroke`, colored like gpui-kit's `Kbd` and labeled by
/// [`key_label`]: a `MINI` tall badge, at least square, rounded by its
/// height, in caption text (ADR-0031). Its debug selector,
/// `key-cap:<keystroke>=<label>` (e.g. `key-cap:escape=Esc`), names both,
/// so tests can tell it from a `Kbd` (`kbd:<keystroke>`) and see the label
/// drawn.
pub fn key_cap(stroke: &Keystroke, cx: &App) -> Div {
    let theme = cx.theme();
    let label = key_label(stroke);
    let selector = format!("key-cap:{}={label}", stroke.unparse());
    div()
        .debug_selector(move || selector)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(height::MINI))
        .min_w(px(height::MINI))
        .px(px(pad::BADGE_X))
        .rounded(px(radius::for_height(height::MINI)))
        .bg(theme.tokens.muted)
        .text_color(theme.muted_foreground)
        .text_style(text::CAPTION)
        .whitespace_normal()
        .child(label)
}
