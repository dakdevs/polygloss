//! The bundled code font, Lilex (OFL-1.1, `assets/fonts/lilex/ofl.txt`,
//! credited in `NOTICE`), design §11.10.
//!
//! Lilex is the default `buffer_font.family`. The four static faces the
//! viewport draws (regular, bold, italic, bold italic: Pierre's syntax
//! styles use weight 700 and italics) are compiled into the binary and
//! registered with GPUI's text system from memory, so the font works without
//! installing anything. Register them before
//! `polygloss_viewport::kit::init_kit` runs: that call checks whether the code
//! font is installed and names it as gpui-kit's monospace family only if it
//! is (else SF Mono).

use std::borrow::Cow;

use gpui_kit::{App, Global};

/// The family name the bundled faces register under.
pub const LILEX_FAMILY: &str = "Lilex";

/// The bundled faces (TrueType), regular first.
pub const LILEX_FONTS: [&[u8]; 4] = [
    include_bytes!("../../../../assets/fonts/lilex/lilex-regular.ttf"),
    include_bytes!("../../../../assets/fonts/lilex/lilex-bold.ttf"),
    include_bytes!("../../../../assets/fonts/lilex/lilex-italic.ttf"),
    include_bytes!("../../../../assets/fonts/lilex/lilex-bold-italic.ttf"),
];

/// Set once the faces are registered with this app's text system.
struct Registered;

impl Global for Registered {}

/// Registers the bundled Lilex faces with the app's text system (once per
/// app; later calls do nothing). Failing to register is logged, not fatal:
/// the viewport then falls back to the system monospaced font.
pub fn register_fonts(cx: &mut App) {
    if cx.has_global::<Registered>() {
        return;
    }
    cx.set_global(Registered);
    let fonts = LILEX_FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("registering the bundled Lilex font: {e:#}");
    }
}
