//! Zed theme JSON model (design §11.10, ADR-0024, ADR-0027) and the built-in
//! themes: Polygloss Light/Dark (the defaults) and Pierre Light/Dark.
//!
//! The format is Zed's theme family: `{ name, author?, themes: [{ name,
//! appearance, style: { <ui color keys>…, syntax: { <capture>: { color,
//! background_color, font_style, font_weight } } } }] }`. Parsing is lenient the
//! way Zed is: unknown keys are ignored at every level, a color that does not
//! parse reads as absent, and a malformed `syntax` entry is skipped. Only the
//! family `name`, a non-empty `themes` list and each theme's `name` and
//! `appearance` are required.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value};

/// An 8-bit sRGB color with alpha, as Zed writes them (`#rrggbbaa`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (hex digits in either
    /// case). Anything else is `None`.
    pub fn parse(s: &str) -> Option<Rgba> {
        let hex = s.strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
        let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 | 4 => {
                let short = |i: usize| digit(i).map(|d| d * 0x11);
                let a = if hex.len() == 4 { short(3)? } else { 0xff };
                Some(Rgba {
                    r: short(0)?,
                    g: short(1)?,
                    b: short(2)?,
                    a,
                })
            }
            6 | 8 => {
                let a = if hex.len() == 8 { pair(6)? } else { 0xff };
                Some(Rgba {
                    r: pair(0)?,
                    g: pair(2)?,
                    b: pair(4)?,
                    a,
                })
            }
            _ => None,
        }
    }

    /// `0xRRGGBBAA`, the form GPUI's `rgba()` takes.
    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes([self.r, self.g, self.b, self.a])
    }
}

/// Whether a theme is meant for a light or a dark system appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    Dark,
}

/// Zed's `font_style` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}

/// One `syntax` entry: how text of a capture is drawn. Every field is optional;
/// absent means "inherit the editor default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SyntaxStyle {
    pub color: Option<Rgba>,
    pub background_color: Option<Rgba>,
    pub font_style: Option<FontStyle>,
    /// CSS-style weight, 100–900 (Zed writes e.g. `700`).
    pub font_weight: Option<u16>,
}

impl SyntaxStyle {
    /// Reads one `syntax` entry leniently: wrong types and unparsable values
    /// become `None`, unknown keys are ignored. `None` if `v` is not an object.
    fn from_value(v: &Value) -> Option<SyntaxStyle> {
        let obj = v.as_object()?;
        let color = |key: &str| obj.get(key).and_then(Value::as_str).and_then(Rgba::parse);
        let font_style = match obj.get("font_style").and_then(Value::as_str) {
            Some("normal") => Some(FontStyle::Normal),
            Some("italic") => Some(FontStyle::Italic),
            Some("oblique") => Some(FontStyle::Oblique),
            _ => None,
        };
        let font_weight = obj
            .get("font_weight")
            .and_then(Value::as_f64)
            .filter(|w| (1.0..=1000.0).contains(w))
            .map(|w| w.round() as u16);
        Some(SyntaxStyle {
            color: color("color"),
            background_color: color("background_color"),
            font_style,
            font_weight,
        })
    }
}

/// A Zed theme family file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ZedThemeFamily {
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    pub themes: Vec<ZedTheme>,
}

/// One theme of a family. `style` holds the UI colors exactly as written (the
/// app maps them to gpui-kit tokens); `syntax` is `style.syntax` lifted out and
/// parsed, so `style` never contains a `syntax` key.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "RawTheme")]
pub struct ZedTheme {
    pub name: String,
    pub appearance: Appearance,
    pub style: Map<String, Value>,
    pub syntax: BTreeMap<String, SyntaxStyle>,
}

#[derive(Deserialize)]
struct RawTheme {
    name: String,
    appearance: Appearance,
    #[serde(default)]
    style: Map<String, Value>,
}

impl From<RawTheme> for ZedTheme {
    fn from(raw: RawTheme) -> ZedTheme {
        let mut style = raw.style;
        let syntax = match style.remove("syntax") {
            Some(Value::Object(entries)) => entries
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), SyntaxStyle::from_value(v)?)))
                .collect(),
            _ => BTreeMap::new(),
        };
        ZedTheme {
            name: raw.name,
            appearance: raw.appearance,
            style,
            syntax,
        }
    }
}

impl ZedTheme {
    /// The UI color `style[key]`, if present and a valid color string. Keys
    /// are Zed's (`editor.background`, `created.background`, …).
    pub fn color(&self, key: &str) -> Option<Rgba> {
        self.style
            .get(key)
            .and_then(Value::as_str)
            .and_then(Rgba::parse)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("invalid Zed theme JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("theme family {0:?} has no themes")]
    NoThemes(String),
}

/// Parses a Zed theme family file.
pub fn load_theme_family(json: &str) -> Result<ZedThemeFamily, ThemeError> {
    let family: ZedThemeFamily = serde_json::from_str(json)?;
    if family.themes.is_empty() {
        return Err(ThemeError::NoThemes(family.name));
    }
    Ok(family)
}

/// Pierre Light as Zed theme JSON: `assets/themes/pierre-light.json`, the
/// output of `scripts/port-pierre-theme.ts` (Apache-2.0 `@pierre/theme`, see
/// `NOTICE`).
pub const PIERRE_LIGHT_JSON: &str = include_str!("../../../assets/themes/pierre-light.json");

/// Pierre Dark as Zed theme JSON: `assets/themes/pierre-dark.json`.
pub const PIERRE_DARK_JSON: &str = include_str!("../../../assets/themes/pierre-dark.json");

/// Polygloss Light as Zed theme JSON: `assets/themes/polygloss-light.json`,
/// our own palette after the redesign reference
/// (`docs/research/redesign-reference.md`), plus the `polygloss.*` keys.
pub const POLYGLOSS_LIGHT_JSON: &str = include_str!("../../../assets/themes/polygloss-light.json");

/// Polygloss Dark as Zed theme JSON: `assets/themes/polygloss-dark.json`.
pub const POLYGLOSS_DARK_JSON: &str = include_str!("../../../assets/themes/polygloss-dark.json");

static PIERRE_LIGHT: LazyLock<ZedTheme> = LazyLock::new(|| builtin(PIERRE_LIGHT_JSON));
static PIERRE_DARK: LazyLock<ZedTheme> = LazyLock::new(|| builtin(PIERRE_DARK_JSON));
static POLYGLOSS_LIGHT: LazyLock<ZedTheme> = LazyLock::new(|| builtin(POLYGLOSS_LIGHT_JSON));
static POLYGLOSS_DARK: LazyLock<ZedTheme> = LazyLock::new(|| builtin(POLYGLOSS_DARK_JSON));

fn builtin(json: &str) -> ZedTheme {
    // The files are committed and the theme tests parse each, so a failure
    // here is a build defect, not an input error.
    let family = load_theme_family(json).expect("built-in theme parses");
    family
        .themes
        .into_iter()
        .next()
        .expect("built-in theme is non-empty")
}

/// The built-in default theme for `appearance`: Polygloss Light or Dark
/// (design §11.10, ADR-0027).
pub fn default_theme(appearance: Appearance) -> &'static ZedTheme {
    match appearance {
        Appearance::Light => &POLYGLOSS_LIGHT,
        Appearance::Dark => &POLYGLOSS_DARK,
    }
}

/// The built-in Pierre Light or Pierre Dark, selectable by name.
pub fn pierre_theme(appearance: Appearance) -> &'static ZedTheme {
    match appearance {
        Appearance::Light => &PIERRE_LIGHT,
        Appearance::Dark => &PIERRE_DARK,
    }
}
