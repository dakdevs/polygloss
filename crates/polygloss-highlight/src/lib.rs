//! Polygloss syntax highlighting: lumis adapter, compact tokens and Zed-format
//! themes. Never links GPUI, git or SQLite.
//!
//! - [`guess_language`] picks a grammar from a path (then a shebang).
//! - [`Highlighter`] runs lumis over a whole blob under a [`Budget`] and a
//!   cancellation flag and returns [`Tokens`]: per-line [`Span`]s of byte
//!   offsets plus a [`StyleId`].
//! - [`load_theme_family`] parses Zed theme JSON (Polygloss Light/Dark, the
//!   defaults, and Pierre Light/Dark are built in: [`default_theme`],
//!   [`pierre_theme`]); [`SyntaxTheme`] maps lumis scopes to styles by longest
//!   dotted prefix.
//! - [`TokenCache`] keeps tokens per `(blob, language, theme)` under a byte
//!   budget.
//!
//! Line numbers are 0-based `u32` and lines end at `\n` (not part of the line;
//! a `\r` before it is content), as in `polygloss_diff::lines`.
#![forbid(unsafe_code)]

pub mod cache;
pub mod highlighter;
pub mod language;
pub mod scope_map;
pub mod theme;
pub mod tokens;

pub use cache::TokenCache;
pub use highlighter::{Budget, HighlightError, Highlighter};
pub use language::{Language, guess_language};
pub use scope_map::{SyntaxTheme, ThemeId};
pub use theme::{
    Appearance, FontStyle, PIERRE_DARK_JSON, PIERRE_LIGHT_JSON, POLYGLOSS_DARK_JSON,
    POLYGLOSS_LIGHT_JSON, Rgba, SyntaxStyle, ThemeError, ZedTheme, ZedThemeFamily, default_theme,
    load_theme_family, pierre_theme,
};
pub use tokens::{Span, StyleId, Tokens};
