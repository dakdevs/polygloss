//! gpui-kit initialization for every window that hosts a [`DiffViewport`],
//! without gpui-kit's startup font scan (plan T2.10.2).
//!
//! The header's ⋯ menu is a gpui-kit `PopupMenu`, so hosts initialize
//! gpui-kit before opening a viewport. `gpui_kit::init` makes the light theme
//! current and, whenever its theme's UI family is `.SystemUIFont` or its
//! mono family is the platform default (`Menlo` on macOS), checks them
//! against every installed family (`TextSystem::all_font_names`): a CoreText
//! scan of every face over XPC, ≈ 440 ms on the dev machine, on the main
//! thread before the first window can open (T2.9 measured it as most of the
//! typical corpus's first paint). [`init_kit`] names both families first, so
//! neither probe runs, and ends in the same theme otherwise.
//!
//! - UI: [`SYSTEM_UI_FONT`], the family GPUI's macOS text system loads for
//!   `.SystemUIFont` (the same font, named directly).
//! - Mono: the code font when it is installed, else the system monospaced
//!   font ([`SYSTEM_MONO_FONT`], SF Mono, which the [`SYSTEM_MONO_ALIASES`]
//!   also name). Never `Menlo` by name: gpui-kit probes (and scans for) its
//!   default whenever it is named.
//!
//! [`DiffViewport`]: crate::DiffViewport

use gpui_kit::component::theme::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, SharedString, TextSystem, font};

/// The family GPUI's macOS text system loads for `.SystemUIFont`.
pub const SYSTEM_UI_FONT: &str = ".AppleSystemUIFont";

/// The system monospaced font (SF Mono) by its system family name.
pub const SYSTEM_MONO_FONT: &str = ".AppleSystemUIFontMonospaced";

/// Code-font names that select [`SYSTEM_MONO_FONT`] (design §11.10): "SF
/// Mono" by name resolves only where Apple's developer fonts are installed.
pub const SYSTEM_MONO_ALIASES: &[&str] = &["SF Mono", "System Mono"];

/// gpui-kit's default mono family on macOS (`TypographyTokens::default()`),
/// which it probes whenever the theme names it.
const KIT_DEFAULT_MONO: &str = "Menlo";

/// The families [`init_kit`] names on gpui-kit's theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitFonts {
    pub ui: SharedString,
    pub mono: SharedString,
}

/// The family a configured code font names: [`SYSTEM_MONO_FONT`] for one of
/// the [`SYSTEM_MONO_ALIASES`], else the name itself.
pub fn code_family(code_font: &str) -> &str {
    if SYSTEM_MONO_ALIASES.contains(&code_font) {
        SYSTEM_MONO_FONT
    } else {
        code_font
    }
}

/// The families to name for a code font `code_font` (`installed`: whether the
/// text system has it; see [`is_installed`]).
pub fn kit_fonts(code_font: &str, installed: bool) -> KitFonts {
    let family = code_family(code_font);
    let mono = if installed && family != KIT_DEFAULT_MONO {
        SharedString::from(family.to_owned())
    } else {
        SharedString::new_static(SYSTEM_MONO_FONT)
    };
    KitFonts {
        ui: SharedString::new_static(SYSTEM_UI_FONT),
        mono,
    }
}

/// Whether `family` resolves to itself rather than to GPUI's fallback stack.
/// Resolves one font; never lists the installed ones.
pub fn is_installed(text_system: &TextSystem, family: &str) -> bool {
    let wanted = font(family.to_owned());
    let id = text_system.resolve_font(&wanted);
    text_system
        .get_font_for_id(id)
        .is_some_and(|resolved| resolved.family == wanted.family)
}

/// `gpui_kit::init` with gpui-kit's fonts named up front ([`kit_fonts`] for
/// `code_font`), so it never scans the installed fonts. Call it once, before
/// opening a window, instead of `gpui_kit::init`.
pub fn init_kit(code_font: &str, cx: &mut App) {
    if !cfg!(target_os = "macos") {
        // The families above are macOS names; elsewhere gpui-kit's probes do
        // what they are for (mapping `.SystemUIFont` to an installed family).
        gpui_kit::init(cx);
        return;
    }
    let installed = is_installed(cx.text_system(), code_font);
    let fonts = kit_fonts(code_font, installed);
    // gpui-kit keeps a theme that exists before its init (only creating one
    // when there is none) and probes only unnamed families.
    cx.set_global(Theme {
        font_family: fonts.ui,
        mono_font_family: fonts.mono,
        ..Theme::default()
    });
    gpui_kit::init(cx);
    // A theme gpui-kit creates starts from its registry's default light and
    // dark themes; give this one the same and load the light one again, as
    // its init does (the named families stay: the themes name none).
    let registry = ThemeRegistry::global(cx);
    let light = registry.default_light_theme().clone();
    let dark = registry.default_dark_theme().clone();
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    Theme::change(ThemeMode::Light, None, cx);
}
