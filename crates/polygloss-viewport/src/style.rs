//! Colors and diff-style settings (design §11.6 "Styles", §11.10).
//!
//! [`ViewportTheme`] resolves everything the viewport paints from one Zed
//! theme: UI colors from `style`, diff colors from `created`/`deleted` and the
//! `version_control.word_*` keys, syntax colors from `syntax` (through
//! [`SyntaxTheme`]). Colors Zed has no key for come from `polygloss.*` style
//! keys, each derived from standard keys when a theme lacks it. Colors are
//! converted to GPUI's [`Hsla`] once, so painting never parses or converts.

use std::sync::Arc;

use gpui_kit::{FontStyle as GpuiFontStyle, FontWeight, Hsla};
use polygloss_highlight::{
    Appearance, FontStyle, Rgba, StyleId, SyntaxTheme, ThemeId, ZedTheme, pierre_theme,
};

/// Change markers (Pierre's diff-style setting, design §11.6 "Styles").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Indicators {
    /// A 3 px bar at the left edge of each changed row's pane (each half's
    /// own edge in split), no glyph.
    #[default]
    Bars,
    /// `+` and `-` glyphs in a column before the code.
    PlusMinus,
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
    /// Backgrounds on, bars, no wrap (design §18).
    fn default() -> DiffStyle {
        DiffStyle {
            backgrounds: true,
            indicators: Indicators::Bars,
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
    /// Behind the file cards, and gap (hidden context) rows (`background`).
    pub canvas: Hsla,
    /// A file card (`editor.background`) and its 1 px border (`border`).
    pub card_background: Hsla,
    pub card_border: Hsla,
    /// Pills: `+a −d`, review state (`element.background`).
    pub pill_background: Hsla,
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
    /// Line numbers of added and removed rows
    /// (`polygloss.created.line_number`, else `created`; likewise deleted).
    pub added_line_number: Hsla,
    pub removed_line_number: Hsla,
    /// Behind those line numbers (`polygloss.created.gutter_background`, else
    /// `created.background` a little stronger; likewise deleted).
    pub added_gutter: Hsla,
    pub removed_gutter: Hsla,
    /// `+a` and `−d` counts (`polygloss.stat.added`, else
    /// `version_control.added`, else `added_accent`; likewise deleted).
    pub stat_added: Hsla,
    pub stat_removed: Hsla,
    /// A commit's short SHA (`polygloss.commit_sha`, else syntax
    /// `type.builtin`, else `terminal.ansi.yellow`).
    pub commit_sha: Hsla,
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
    /// Behind every find match (`terminal.ansi.yellow`, translucent, like the
    /// find bar's result list).
    pub find_match: Hsla,
    /// Behind the current find match (`search.active_match_background`, else
    /// a stronger yellow).
    pub find_match_current: Hsla,
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
        let border = pick(&["border", "border.variant"], border);
        let subheader = pick(
            &["editor.subheader.background", "surface.background"],
            surface,
        );
        let added_accent = pick(&["created", "version_control.added"], 0x18a46cff);
        let removed_accent = pick(&["deleted", "version_control.deleted"], 0xd52c36ff);
        let added_background = t
            .color("created.background")
            .map_or(added_accent.opacity(0.2), hsla);
        let removed_background = t
            .color("deleted.background")
            .map_or(removed_accent.opacity(0.2), hsla);
        let or = |key: &str, fallback: Hsla| t.color(key).map_or(fallback, hsla);
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
        // Yellow, as the find bar's result list marks matches (gpui-kit's
        // yellow comes from `terminal.ansi.yellow` too): it stays visible on
        // added and removed rows, where a translucent accent blue fades.
        let yellow = pick(&["terminal.ansi.yellow", "warning"], 0xffca00ff);
        let commit_sha = t
            .color("polygloss.commit_sha")
            .or_else(|| t.syntax.get("type.builtin").and_then(|s| s.color))
            .map_or(yellow, hsla);
        let find_match = yellow.opacity(0.35);
        let find_match_current = t
            .color("search.active_match_background")
            .map_or(yellow.opacity(0.8), hsla);
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
            border,
            header_background: subheader,
            header_foreground: pick(&["text", "editor.foreground"], fg),
            canvas: or("background", subheader),
            card_background: background,
            card_border: border,
            pill_background: badge_background,
            empty_cell: subheader.opacity(0.5),
            added_background,
            removed_background,
            added_word: t
                .color("version_control.word_added")
                .map_or(added_accent.opacity(0.4), hsla),
            removed_word: t
                .color("version_control.word_deleted")
                .map_or(removed_accent.opacity(0.4), hsla),
            added_line_number: or("polygloss.created.line_number", added_accent),
            removed_line_number: or("polygloss.deleted.line_number", removed_accent),
            added_gutter: or(
                "polygloss.created.gutter_background",
                stronger(added_background, added_accent),
            ),
            removed_gutter: or(
                "polygloss.deleted.gutter_background",
                stronger(removed_background, removed_accent),
            ),
            stat_added: or(
                "polygloss.stat.added",
                or("version_control.added", added_accent),
            ),
            stat_removed: or(
                "polygloss.stat.deleted",
                or("version_control.deleted", removed_accent),
            ),
            commit_sha,
            added_accent,
            removed_accent,
            accent,
            hover,
            badge_background,
            cursor_line,
            selection,
            find_match,
            find_match_current,
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

/// `tint` slightly stronger: `accent` at 10% composited over it ("over",
/// alpha included), so a translucent tint gains opacity and an opaque one
/// leans toward the accent. Polygloss Light's sampled gutter is about this
/// far from its row tint.
fn stronger(tint: Hsla, accent: Hsla) -> Hsla {
    const SHARE: f32 = 0.1;
    let (bg, fg) = (gpui_kit::Rgba::from(tint), gpui_kit::Rgba::from(accent));
    let fg_a = fg.a * SHARE;
    let a = fg_a + bg.a * (1.0 - fg_a);
    if a <= 0.0 {
        return tint;
    }
    let mix = |f: f32, b: f32| (f * fg_a + b * bg.a * (1.0 - fg_a)) / a;
    gpui_kit::Rgba {
        r: mix(fg.r, bg.r),
        g: mix(fg.g, bg.g),
        b: mix(fg.b, bg.b),
        a,
    }
    .into()
}
