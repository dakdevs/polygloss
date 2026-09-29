//! Fenced code in comments, highlighted with lumis (design §8.7, §11.11):
//! the fence's info string names the grammar (`rust`, `ts`, `py`, …;
//! `polygloss_highlight::Language::from_name`), colors come from the active
//! theme's syntax styles, as in the diff. Unknown languages stay plain.
//!
//! gpui-kit keeps a code block's highlights while its text view gets the
//! same highlighter `Arc`, so [`highlighter`] hands out one per theme rather
//! than a fresh closure every frame.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use gpui_kit::base::text::CodeBlock;
use gpui_kit::{App, Global, HighlightStyle};
use polygloss_highlight::{Budget, Highlighter, Language, StyleId};
use polygloss_viewport::ViewportTheme;

/// What gpui-kit's text views call for each fenced code block.
pub type HighlighterFn = dyn Fn(&CodeBlock) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync;

/// A comment's code blocks are small; anything larger stays plain.
const BUDGET: Budget = Budget {
    time: Duration::from_millis(100),
    max_lines: 5_000,
};

/// The highlighter of the active theme (`theme::viewport_theme`).
struct Current {
    theme: Arc<ViewportTheme>,
    highlighter: Arc<HighlighterFn>,
}

impl Global for Current {}

/// The highlighter for the active theme, shared by every text view until
/// the theme changes.
pub fn highlighter(cx: &mut App) -> Arc<HighlighterFn> {
    let theme = crate::theme::viewport_theme(cx);
    if let Some(current) = cx.try_global::<Current>()
        && Arc::ptr_eq(&current.theme, &theme)
    {
        return current.highlighter.clone();
    }
    let for_blocks = theme.clone();
    let highlighter: Arc<HighlighterFn> = Arc::new(move |block: &CodeBlock| {
        highlight(&block.code(), block.lang().as_deref(), &for_blocks)
    });
    cx.set_global(Current {
        theme,
        highlighter: highlighter.clone(),
    });
    highlighter
}

/// Syntax highlights of `code` in `lang` (a fence info string) with
/// `theme`'s colors: byte ranges into `code`, sorted, on char boundaries.
/// Empty for no language, an unknown one, or code over the budget.
pub fn highlight(
    code: &str,
    lang: Option<&str>,
    theme: &ViewportTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(lang) = lang
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .and_then(Language::from_name)
    else {
        return Vec::new();
    };
    let cancel = AtomicUsize::new(0);
    let Ok(tokens) =
        Highlighter::new(theme.syntax.clone()).highlight(code.as_bytes(), &lang, &cancel, BUDGET)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut line_start = 0usize;
    for (i, line) in code.split('\n').enumerate() {
        for span in tokens.line(i as u32) {
            if span.style == StyleId::DEFAULT {
                continue;
            }
            let start = line_start + span.start as usize;
            let end = start + span.len as usize;
            if end > code.len() || !code.is_char_boundary(start) || !code.is_char_boundary(end) {
                continue;
            }
            let style = theme.token_style(span.style);
            out.push((
                start..end,
                HighlightStyle {
                    color: Some(style.color),
                    font_weight: Some(style.weight),
                    font_style: Some(style.style),
                    ..HighlightStyle::default()
                },
            ));
        }
        line_start += line.len() + 1;
    }
    out
}
