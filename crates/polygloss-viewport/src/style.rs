//! Colors and diff-style settings (design §11.6 "Styles", §11.10).
//!
//! [`ViewportTheme`] resolves everything the viewport paints from one Zed
//! theme: UI colors from `style`, diff colors from `created`/`deleted` and the
//! `version_control.word_*` keys, syntax colors from `syntax` (through
//! [`SyntaxTheme`]). Colors are converted to GPUI's [`Hsla`] once, so painting
//! never parses or converts.

use std::sync::Arc;

use gpui_kit::{FontStyle as GpuiFontStyle, FontWeight, Hsla};
use polygloss_highlight::{
    Appearance, FontStyle, Rgba, StyleId, SyntaxTheme, ThemeId, ZedTheme, pierre_theme,
};

/// Change markers next to the code (Pierre's diff-style setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Indicators {
    /// `+` and `-` glyphs in a column before the code.
    #[default]
    PlusMinus,
    /// A thin colored bar at the code's left edge.
    Bars,
    /// No markers (the backgrounds, if on, still show the change).
    None,
}

/// Pierre's diff-style settings (design §11.6, settings `diff.style.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiffStyle {
    /// Tint removed and added rows.
    pub backgrounds: bool,
    pub indicators: Indicators,
    /// Wrap long lines at the code column's width instead of cutting them.
    /// A split row is as tall as its taller side.
    pub wrap: bool,
}

impl Default for DiffStyle {
    /// Backgrounds on, `+`/`-` indicators, no wrap (design §18).
    fn default() -> DiffStyle {
        DiffStyle {
            backgrounds: true,
            indicators: Indicators::PlusMinus,
            wrap: false,
        }
    }
}

/// Every color the viewport paints, resolved from a Zed theme.
#[derive(Debug, Clone)]
pub struct ViewportTheme {
    pub name: String,
    pub appearance: Appearance,
    /// Behind everything (`editor.background`).
    pub background: Hsla,
    /// Unstyled code (`editor.foreground`).
    pub foreground: Hsla,
    /// Line numbers (`editor.line_number`).
    pub line_number: Hsla,
    /// Secondary text: gap labels, placeholders (`text.muted`).
    pub muted: Hsla,
    /// Separators between files and between split columns (`border`).
    pub border: Hsla,
    /// File header strip (`editor.subheader.background`).
    pub header_background: Hsla,
    /// File header text (`text`).
    pub header_foreground: Hsla,
    /// Gap (hidden context) rows.
    pub gap_background: Hsla,
    /// The empty side of an unbalanced split row.
    pub empty_cell: Hsla,
    /// Added and removed rows (`created.background` / `deleted.background`).
    pub added_background: Hsla,
    pub removed_background: Hsla,
    /// Changed words on paired lines (`version_control.word_added` / `_deleted`).
    pub added_word: Hsla,
    pub removed_word: Hsla,
    /// `+`/`-` glyphs and bars (`created` / `deleted`).
    pub added_accent: Hsla,
    pub removed_accent: Hsla,
    /// Clickable text (gap expanders, "Load diff") and a checked Viewed box
    /// (`text.accent`).
    pub accent: Hsla,
    /// Behind a control under the pointer (`ghost_element.hover`).
    pub hover: Hsla,
    /// Header badges: mode, binary, generated, … (`element.background`).
    pub badge_background: Hsla,
    /// Behind the line cursor's row (`editor.active_line.background`).
    pub cursor_line: Hsla,
    /// Behind the lines of a range and selected text (Zed's
    /// `players[0].selection`).
    pub selection: Hsla,
    /// Syntax styles for the highlighter; token `StyleId`s index into it.
    pub syntax: Arc<SyntaxTheme>,
    /// Per `StyleId`: color (the foreground when the style has none), weight
    /// and slant.
    syntax_styles: Vec<TokenStyle>,
}

/// How a syntax token is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenStyle {
    pub color: Hsla,
    pub weight: FontWeight,
    pub style: GpuiFontStyle,
}

impl ViewportTheme {
    /// Resolves `t`, falling back to neutral colors for keys it lacks.
    pub fn from_zed(t: &ZedTheme) -> ViewportTheme {
        let dark = t.appearance == Appearance::Dark;
        let pick = |keys: &[&str], fallback: u32| {
            keys.iter()
                .find_map(|k| t.color(k))
                .map_or_else(|| gpui_kit::rgba(fallback).into(), hsla)
        };
        let (bg, fg, muted, surface, border) = if dark {
            (0x0a0a0aff, 0xfafafaff, 0xa3a3a3ff, 0x171717ff, 0x262626ff)
        } else {
            (0xffffffff, 0x0a0a0aff, 0x737373ff, 0xf5f5f5ff, 0xe5e5e5ff)
        };
        let background = pick(&["editor.background", "background"], bg);
        let foreground = pick(&["editor.foreground", "text"], fg);
        let muted = pick(&["text.muted", "editor.line_number"], muted);
        let subheader = pick(
            &["editor.subheader.background", "surface.background"],
            surface,
        );
        let added_accent = pick(&["created", "version_control.added"], 0x18a46cff);
        let removed_accent = pick(&["deleted", "version_control.deleted"], 0xd52c36ff);
        let accent = pick(
            &["text.accent", "link_text.hover", "icon.accent"],
            0x009fffff,
        );
        let hover = t
            .color("ghost_element.hover")
            .or_else(|| t.color("element.hover"))
            .map_or(accent.opacity(0.15), hsla);
        let badge_background = t
            .color("element.background")
            .map_or(subheader.blend(foreground.opacity(0.06)), hsla);
        let cursor_line = t
            .color("editor.active_line.background")
            .map_or(accent.opacity(0.1), hsla);
        // The local player's selection (Zed's `players[0].selection`).
        let selection = t
            .style
            .get("players")
            .and_then(|p| p.as_array())
            .and_then(|p| p.first())
            .and_then(|p| p.get("selection"))
            .and_then(|c| c.as_str())
            .and_then(Rgba::parse)
            .map_or(accent.opacity(0.18), hsla);
        let syntax = Arc::new(SyntaxTheme::from_zed(t));
        let syntax_styles = syntax
            .styles()
            .iter()
            .map(|s| TokenStyle {
                color: s.color.map_or(foreground, hsla),
                weight: s
                    .font_weight
                    .map_or(FontWeight::NORMAL, |w| FontWeight(f32::from(w))),
                style: match s.font_style {
                    Some(FontStyle::Italic) => GpuiFontStyle::Italic,
                    Some(FontStyle::Oblique) => GpuiFontStyle::Oblique,
                    Some(FontStyle::Normal) | None => GpuiFontStyle::Normal,
                },
            })
            .collect();
        ViewportTheme {
            name: t.name.clone(),
            appearance: t.appearance,
            background,
            foreground,
            line_number: pick(&["editor.line_number", "text.muted"], 0x737373ff),
            muted,
            border: pick(&["border", "border.variant"], border),
            header_background: subheader,
            header_foreground: pick(&["text", "editor.foreground"], fg),
            gap_background: subheader,
            empty_cell: subheader.opacity(0.5),
            added_background: t
                .color("created.background")
                .map_or(added_accent.opacity(0.2), hsla),
            removed_background: t
                .color("deleted.background")
                .map_or(removed_accent.opacity(0.2), hsla),
            added_word: t
                .color("version_control.word_added")
                .map_or(added_accent.opacity(0.4), hsla),
            removed_word: t
                .color("version_control.word_deleted")
                .map_or(removed_accent.opacity(0.4), hsla),
            added_accent,
            removed_accent,
            accent,
            hover,
            badge_background,
            cursor_line,
            selection,
            syntax,
            syntax_styles,
        }
    }

    /// The built-in Pierre Light or Pierre Dark (design §11.10).
    pub fn pierre(appearance: Appearance) -> ViewportTheme {
        ViewportTheme::from_zed(pierre_theme(appearance))
    }

    /// The highlighter theme's identity: tokens are only valid for the theme
    /// they were computed with.
    pub fn syntax_id(&self) -> ThemeId {
        self.syntax.id()
    }

    /// How tokens of `id` are drawn; unknown ids draw as plain text.
    pub fn token_style(&self, id: StyleId) -> TokenStyle {
        self.syntax_styles
            .get(id.0 as usize)
            .copied()
            .unwrap_or(TokenStyle {
                color: self.foreground,
                weight: FontWeight::NORMAL,
                style: GpuiFontStyle::Normal,
            })
    }
}

impl Default for ViewportTheme {
    /// Pierre Light.
    fn default() -> ViewportTheme {
        ViewportTheme::pierre(Appearance::Light)
    }
}

/// A theme color as GPUI's [`Hsla`].
pub fn hsla(c: Rgba) -> Hsla {
    gpui_kit::rgba(c.to_u32()).into()
}
