//! ` ```suggestion ` blocks (design §8.5): a fenced block whose info
//! string's first word is `suggestion` proposes a replacement for the
//! thread's anchored **new-side** lines. On such threads it renders as a
//! mini-diff, GitHub-style: the anchored lines removed, the suggested ones
//! added. On old-side and file anchors ([`context_for`] is `None`) no plugin
//! is installed and gpui-kit shows it as a plain code block. There is no
//! Apply button in v1; MCP returns suggestions structurally
//! (`polygloss_core::review::parse_suggestions`, same parser).

use std::sync::Arc;

use gpui_kit::base::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextView, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, px,
};
use polygloss_core::review::{Subject, ThreadAnchor};
use polygloss_diff::Side;

/// Lines of context `anchor_snippet` holds above the anchored lines (core's
/// `SNIPPET_CONTEXT`).
const SNIPPET_CONTEXT: u32 = 3;

/// What a suggestion replaces: the anchored new-side lines, first at
/// `start_line` (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionContext {
    pub start_line: u32,
    pub lines: Arc<[String]>,
}

/// The suggestion context of a thread anchored at `anchor`: new-side line
/// subjects only, with the lines as they were when it was created (from its
/// snippet; for a moved thread they are the current lines too).
pub fn context_for(anchor: &ThreadAnchor) -> Option<SuggestionContext> {
    let Subject::Line {
        side: Side::New,
        start_line,
        line,
        ..
    } = anchor.subject
    else {
        return None;
    };
    let lines = anchored_lines(anchor.anchor_snippet.as_deref()?, start_line, line)?;
    Some(SuggestionContext {
        start_line,
        lines: lines.into(),
    })
}

/// Lines `start_line..=line` (1-based) out of `snippet`, which holds lines
/// `max(1, start_line - 3)..` joined with `\n`. `None` when the snippet is
/// too short or the range is reversed.
pub fn anchored_lines(snippet: &str, start_line: u32, line: u32) -> Option<Vec<String>> {
    if start_line == 0 || line < start_line {
        return None;
    }
    let first = start_line.saturating_sub(SNIPPET_CONTEXT).max(1);
    let skip = (start_line - first) as usize;
    let count = (line - start_line + 1) as usize;
    let lines: Vec<&str> = snippet.split('\n').collect();
    let picked = lines.get(skip..skip + count)?;
    Some(picked.iter().map(|l| (*l).to_owned()).collect())
}

/// A row of the mini-diff, with its line number on its side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniRow {
    Removed { line: u32, text: SharedString },
    Added { line: u32, text: SharedString },
}

/// The mini-diff of `replacement` for `ctx`: every anchored line removed,
/// then every suggested line added (an empty suggestion deletes the lines).
pub fn mini_diff(ctx: &SuggestionContext, replacement: &str) -> Vec<MiniRow> {
    let removed = ctx.lines.iter().enumerate().map(|(i, l)| MiniRow::Removed {
        line: ctx.start_line + i as u32,
        text: l.clone().into(),
    });
    let added: Vec<MiniRow> = if replacement.is_empty() {
        Vec::new()
    } else {
        replacement
            .split('\n')
            .enumerate()
            .map(|(i, l)| MiniRow::Added {
                line: ctx.start_line + i as u32,
                text: l.trim_end_matches('\r').to_owned().into(),
            })
            .collect()
    };
    removed.chain(added).collect()
}

/// The replacement text of a parsed suggestion block.
struct Suggestion {
    replacement: String,
}

/// Claims ` ```suggestion ` blocks and renders them against `ctx`.
pub struct SuggestionPlugin {
    ctx: SuggestionContext,
}

impl MarkdownPlugin for SuggestionPlugin {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "polygloss-suggestion"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Code(code) = node else {
            return None;
        };
        if code.lang.as_deref() != Some("suggestion") {
            return None;
        }
        Some(
            MarkdownNode::new(
                "polygloss-suggestion",
                Suggestion {
                    replacement: code.value.clone(),
                },
            )
            .text(code.value.clone()),
        )
    }

    fn render(&self, node: &MarkdownNode, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let replacement = node
            .data::<Suggestion>()
            .map_or("", |s| s.replacement.as_str());
        render_mini_diff(&self.ctx, replacement, cx)
    }
}

/// `view` with suggestion blocks shown as mini-diffs against `ctx`; `None`
/// (old-side and file anchors) leaves them plain code blocks.
pub fn with_suggestions(view: TextView, ctx: Option<SuggestionContext>) -> TextView {
    match ctx {
        Some(ctx) => view.plugin(SuggestionPlugin { ctx }),
        None => view,
    }
}

/// The mini-diff element: a "Suggested change" header, then the removed and
/// added lines with their numbers and `-`/`+` markers in the diff colors.
fn render_mini_diff(ctx: &SuggestionContext, replacement: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let diff = crate::theme::viewport_theme(cx);
    let (family, size) = super::code_font(cx);
    let rows = mini_diff(ctx, replacement);
    let widest = rows
        .iter()
        .map(|r| match r {
            MiniRow::Removed { line, .. } | MiniRow::Added { line, .. } => *line,
        })
        .max()
        .unwrap_or(1);
    let gutter = px(size * 0.62 * (widest.to_string().len() as f32 + 1.0) + 12.0);
    let (mut removed_ix, mut added_ix) = (0usize, 0usize);
    v_flex()
        .debug_selector(|| "suggestion-diff".into())
        .my_1()
        .w_full()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(
            h_flex()
                .px_2()
                .py_1()
                .gap_1()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Suggested change"),
        )
        .children(rows.into_iter().map(|row| {
            let (line, text, removed) = match row {
                MiniRow::Removed { line, text } => (line, text, true),
                MiniRow::Added { line, text } => (line, text, false),
            };
            let selector = if removed {
                removed_ix += 1;
                format!("suggestion-removed-{}", removed_ix - 1)
            } else {
                added_ix += 1;
                format!("suggestion-added-{}", added_ix - 1)
            };
            let (bg, accent) = if removed {
                (diff.removed_background, diff.removed_accent)
            } else {
                (diff.added_background, diff.added_accent)
            };
            h_flex()
                .debug_selector(move || selector.clone())
                .w_full()
                .items_start()
                .bg(bg)
                .font_family(family.clone())
                .text_size(px(size))
                .line_height(px((size * 1.54).round()))
                .child(
                    div()
                        .flex_none()
                        .w(gutter)
                        .pr_2()
                        .flex()
                        .justify_end()
                        .text_color(diff.line_number)
                        .child(line.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(size * 0.62 + 6.0))
                        .text_color(accent)
                        .child(if removed { "-" } else { "+" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pr_2()
                        .text_color(diff.foreground)
                        .when(text.is_empty(), |el| el.child(" "))
                        .when(!text.is_empty(), |el| el.child(text)),
                )
        }))
}
