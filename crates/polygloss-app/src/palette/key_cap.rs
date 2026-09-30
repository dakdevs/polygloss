//! How the palette and the cheat sheet show a key: gpui-kit's `Kbd`
//! spelling (`⌘K`, `⇧⏎`), except that Escape reads `Esc`. Its `⎋` glyph is
//! easy to mistake for a reload arrow.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    AnyElement, App, Half as _, InteractiveElement as _, IntoElement as _, Keystroke,
    ParentElement as _, Styled as _, div, relative,
};

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

/// A key cap for `stroke`, styled like gpui-kit's `Kbd`, labeled by
/// [`key_label`]. Its debug selector, `key-cap:<keystroke>=<label>` (e.g.
/// `key-cap:escape=Esc`), names both, so tests can tell it from a `Kbd`
/// (`kbd:<keystroke>`) and see the label drawn.
pub fn key_cap(stroke: &Keystroke, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = key_label(stroke);
    let selector = format!("key-cap:{}={label}", stroke.unparse());
    div()
        .debug_selector(move || selector)
        .text_color(theme.muted_foreground)
        .bg(theme.tokens.muted)
        .py_0p5()
        .px_1()
        .min_w_5()
        .text_center()
        .rounded(theme.radius.half())
        .line_height(relative(1.))
        .text_xs()
        .whitespace_normal()
        .flex_shrink_0()
        .child(label)
        .into_any_element()
}
