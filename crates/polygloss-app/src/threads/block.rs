//! The thread block (design §8.2, §8.4, §8.6, §11.6): a GitHub-style card
//! under the anchored line with the root comment and its flat replies, each
//! with its author ("You", or the agent's name with an Agent badge), a
//! Draft badge while unpublished, and the rendered markdown body. Thread
//! badges sit in a header: Question, Outdated (with the original snippet
//! the thread was written against), Note, Resolved. Resolved threads and
//! agent notes start as a one-line chip that opens on click.
//!
//! Rendered from [`ReviewThreads`] each time the viewport paints the block,
//! so it always shows the current thread; the model invalidates the block
//! when its content changes. Debug selectors (`thread-<id>`,
//! `thread-chip-<id>`, `thread-comment-<comment>`, `thread-agent-<comment>`,
//! `thread-draft-<comment>`, `thread-question-<id>`, `thread-outdated-<id>`,
//! `thread-snippet-<id>`) are what the tests look for.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use polygloss_core::review::{
    AuthorKind, CommentView, Position, PositionState, Subject, ThreadKind, ThreadStatus, ThreadView,
};

use super::ReviewThreads;
use crate::markdown::{self, suggestion};

/// Space around a block's card inside its row.
const MARGIN_X: f32 = 10.0;
const MARGIN_Y: f32 = 6.0;
/// The avatar's size; comment bodies are indented past it.
const AVATAR: f32 = 20.0;

/// Thread `id`'s block: its card, or its chip while collapsed.
pub fn render(
    model: &Entity<ReviewThreads>,
    id: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (thread, collapsed, position) = {
        let m = model.read(cx);
        let Some(thread) = m.thread_arc(id) else {
            return div().into_any_element();
        };
        let collapsed = m.collapsed(&thread);
        (thread, collapsed, m.position(id).cloned())
    };
    let inner = if collapsed {
        chip(model, &thread, cx).into_any_element()
    } else {
        card(model, &thread, position.as_ref(), window, cx)
    };
    div()
        .w_full()
        .px(px(MARGIN_X))
        .py(px(MARGIN_Y))
        // Opaque to clicks (the diff under it does not see them), but the
        // wheel still scrolls the diff.
        .block_mouse_except_scroll()
        .child(inner)
        .into_any_element()
}

/// A small outlined pill.
pub(crate) fn pill(text: impl Into<SharedString>, color: Hsla) -> gpui_kit::Div {
    div()
        .flex_none()
        .px_1p5()
        .rounded_full()
        .border_1()
        .border_color(color.opacity(0.55))
        .text_xs()
        .line_height(px(16.))
        .text_color(color)
        .child(text.into())
}

/// The first line of a body as plain text, for chips and the panel.
pub(crate) fn excerpt(body: &str) -> String {
    let line = body
        .lines()
        .map(|l| l.trim_start_matches(['#', '>', '-', '*', ' ', '\t']).trim())
        .find(|l| !l.is_empty() && !l.starts_with("```"))
        .unwrap_or("");
    let plain: String = line
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect();
    if plain.chars().count() > 140 {
        plain.chars().take(140).collect::<String>() + "…"
    } else {
        plain
    }
}

/// How an author is shown.
pub(crate) fn author_name(kind: AuthorKind, name: &str) -> SharedString {
    match kind {
        AuthorKind::Human => "You".into(),
        AuthorKind::Agent => name.to_owned().into(),
    }
}

fn avatar(kind: AuthorKind, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (bg, fg, icon) = match kind {
        AuthorKind::Human => (theme.secondary, theme.secondary_foreground, IconName::User),
        AuthorKind::Agent => (theme.primary, theme.primary_foreground, IconName::Bot),
    };
    div()
        .flex_none()
        .size(px(AVATAR))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(bg)
        .text_color(fg)
        .child(Icon::new(icon).xsmall())
}

/// "Line 12", "Lines 12–14" (1-based, as created), "File" or "Review".
pub(crate) fn subject_label(subject: &Subject) -> String {
    match subject {
        Subject::Line {
            start_line, line, ..
        } if start_line < line => format!("Lines {start_line}–{line}"),
        Subject::Line { line, .. } => format!("Line {line}"),
        Subject::File { .. } => "File".to_owned(),
        Subject::Review => "Review".to_owned(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A resolved thread or an agent note, collapsed to one line.
fn chip(model: &Entity<ReviewThreads>, thread: &Arc<ThreadView>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let id = thread.id.clone();
    let root = thread.comments.first();
    let (icon, color, label) = if thread.status == ThreadStatus::Resolved {
        (IconName::CircleCheck, theme.success, "Resolved".to_owned())
    } else {
        (
            IconName::Bot,
            theme.primary,
            format!("{} note", thread.created_by.name),
        )
    };
    let replies = thread.comments.len().saturating_sub(1);
    let model = model.clone();
    let toggle_id = id.clone();
    h_flex()
        .id(SharedString::from(format!("thread-chip-{id}")))
        .debug_selector(move || format!("thread-chip-{id}"))
        .w_full()
        .h(px(30.))
        .px_2p5()
        .gap_2()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .text_xs()
        .cursor_pointer()
        .hover(|s| s.bg(theme.secondary_hover))
        .on_click(move |_, _, cx| {
            model.update(cx, |m, cx| m.toggle_expanded(&toggle_id, cx));
        })
        .child(Icon::new(icon).xsmall().text_color(color))
        .child(
            div()
                .flex_none()
                .font_medium()
                .text_color(theme.foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(root.map(|c| excerpt(&c.body_md)).unwrap_or_default()),
        )
        .when(replies > 0, |el| {
            el.child(
                div()
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .child(match replies {
                        1 => "1 reply".to_owned(),
                        n => format!("{n} replies"),
                    }),
            )
        })
        .child(
            Icon::new(IconName::ChevronDown)
                .xsmall()
                .text_color(theme.muted_foreground),
        )
}

/// The expanded thread: header badges, the outdated snippet, the comments.
pub(crate) fn card(
    model: &Entity<ReviewThreads>,
    thread: &Arc<ThreadView>,
    position: Option<&Position>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = thread.id.clone();
    let outdated = position.is_some_and(|p| p.state == PositionState::Outdated);
    let collapsible = thread.status == ThreadStatus::Resolved || thread.kind == ThreadKind::Note;
    let header = header(model, thread, outdated, collapsible, cx);
    let snippet = outdated.then(|| snippet(thread, cx));
    let ctx = suggestion::context_for(&thread.anchor, position);
    let comments: Vec<AnyElement> = thread
        .comments
        .iter()
        .enumerate()
        .map(|(i, c)| comment(model, c, i > 0, ctx.clone(), window, cx).into_any_element())
        .collect();
    // Reply box and Resolve (T3.10), when the tab has composers.
    let footer = crate::composer::card_footer(model.read(cx), thread, cx);
    let theme = cx.theme();
    v_flex()
        .id(SharedString::from(format!("thread-{id}")))
        .debug_selector(move || format!("thread-{id}"))
        .w_full()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .shadow_xs()
        .overflow_hidden()
        .when_some(header, |el, h| el.child(h))
        .when_some(snippet, |el, s| el.child(s))
        .children(comments)
        .when_some(footer, |el, f| el.child(f))
        .into_any_element()
}

/// The badges of a thread, or `None` for a plain open comment thread.
fn header(
    model: &Entity<ReviewThreads>,
    thread: &Arc<ThreadView>,
    outdated: bool,
    collapsible: bool,
    cx: &App,
) -> Option<impl IntoElement + use<>> {
    let theme = cx.theme();
    let question = thread.kind == ThreadKind::Question;
    let note = thread.kind == ThreadKind::Note;
    let resolved = thread.status == ThreadStatus::Resolved;
    if !(outdated || question || note || resolved) {
        return None;
    }
    let id = thread.id.clone();
    let resolved_by = thread.resolved_by.as_ref().map(|r| match r.kind {
        AuthorKind::Human => "you".to_owned(),
        AuthorKind::Agent => r.name.clone().unwrap_or_else(|| "an agent".to_owned()),
    });
    let model = model.clone();
    let (q_id, o_id) = (id.clone(), id.clone());
    Some(
        h_flex()
            .w_full()
            .min_h(px(30.))
            .px_3()
            .py_1()
            .gap_1p5()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .text_xs()
            .when(resolved, |el| {
                el.child(
                    Icon::new(IconName::CircleCheck)
                        .xsmall()
                        .text_color(theme.success),
                )
                .child(
                    div()
                        .text_color(theme.foreground)
                        .child(match &resolved_by {
                            Some(by) => format!("Resolved by {by}"),
                            None => "Resolved".to_owned(),
                        }),
                )
            })
            .when(note, |el| el.child(pill("Note", theme.muted_foreground)))
            .when(question, |el| {
                el.child(
                    pill("Question", theme.info)
                        .debug_selector(move || format!("thread-question-{q_id}")),
                )
            })
            .when(outdated, |el| {
                el.child(
                    pill("Outdated", theme.warning)
                        .debug_selector(move || format!("thread-outdated-{o_id}")),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(subject_label(&thread.anchor.subject)),
            )
            .when(collapsible, |el| {
                el.child(
                    Button::new(SharedString::from(format!("thread-hide-{id}")))
                        .xsmall()
                        .ghost()
                        .icon(IconName::ChevronUp)
                        .tooltip("Collapse")
                        .on_click(move |_, _, cx| {
                            model.update(cx, |m, cx| m.toggle_expanded(&id, cx));
                        }),
                )
            }),
    )
}

/// An outdated thread's original lines (`anchor_snippet`: the anchored
/// lines with 3 lines of context), the anchored ones tinted.
fn snippet(thread: &ThreadView, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let diff = crate::theme::viewport_theme(cx);
    let (family, size) = markdown::code_font(cx);
    let id = thread.id.clone();
    let (start, end) = match thread.anchor.subject {
        Subject::Line {
            start_line, line, ..
        } => (start_line, line),
        _ => (1, 0),
    };
    let first = start.saturating_sub(3).max(1);
    let text = thread.anchor.anchor_snippet.clone().unwrap_or_default();
    let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let digits = (first as usize + lines.len()).to_string().len() as f32;
    let gutter = px(size * 0.62 * (digits + 1.0) + 10.0);
    v_flex()
        .debug_selector(move || format!("thread-snippet-{id}"))
        .w_full()
        .py_1()
        .border_b_1()
        .border_color(theme.border)
        .bg(diff.background)
        .font_family(family)
        .font_features(markdown::code_font_features(cx))
        .text_size(px(size - 1.0))
        .line_height(px(((size - 1.0) * 1.54).round()))
        .children(lines.into_iter().enumerate().map(move |(i, l)| {
            let n = first + i as u32;
            let anchored = n >= start && n <= end;
            h_flex()
                .w_full()
                .when(anchored, |el| el.bg(diff.removed_background))
                .child(
                    div()
                        .flex_none()
                        .w(gutter)
                        .pr_2()
                        .flex()
                        .justify_end()
                        .text_color(diff.line_number)
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(diff.foreground)
                        .child(if l.is_empty() { " ".to_owned() } else { l }),
                )
        }))
}

/// One comment: avatar, author, badges, time, Edit and Delete for one's
/// own (T3.10), then the body (or its edit composer).
fn comment(
    model: &Entity<ReviewThreads>,
    c: &CommentView,
    reply: bool,
    ctx: Option<suggestion::SuggestionContext>,
    _window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let agent = c.author.kind == AuthorKind::Agent;
    let tools = crate::composer::comment_tools(model.read(cx), c, cx);
    let editor = crate::composer::comment_editor(model.read(cx), c, cx);
    let body: AnyElement = if let Some(editor) = editor {
        editor
    } else if c.deleted {
        div()
            .italic()
            .text_color(cx.theme().muted_foreground)
            .child("Comment deleted")
            .into_any_element()
    } else {
        let view = markdown::render_markdown(
            SharedString::from(format!("comment-{}", c.id)),
            &c.body_md,
            cx,
        );
        suggestion::with_suggestions(view, ctx).into_any_element()
    };
    let theme = cx.theme();
    let now = now_ms();
    let when = crate::home::row::relative_time(
        now,
        c.created_at,
        crate::home::row::local_utc_offset_s(now),
    );
    let when = if c.edited_at.is_some() {
        format!("{when} · edited")
    } else {
        when
    };
    let (cid, agent_id, draft_id) = (c.id.clone(), c.id.clone(), c.id.clone());
    v_flex()
        .debug_selector(move || format!("thread-comment-{cid}"))
        .w_full()
        .px_3()
        .py_2()
        .gap_1()
        .when(reply, |el| el.border_t_1().border_color(theme.border))
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(avatar(c.author.kind, cx))
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(author_name(c.author.kind, &c.author.name)),
                )
                .when(agent, |el| {
                    el.child(
                        pill("Agent", theme.primary)
                            .debug_selector(move || format!("thread-agent-{agent_id}")),
                    )
                })
                .when(c.draft, |el| {
                    el.child(
                        pill("Draft", theme.warning)
                            .debug_selector(move || format!("thread-draft-{draft_id}")),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(when),
                )
                .when_some(tools, |el, tools| el.child(div().flex_1()).child(tools)),
        )
        .child(
            div()
                .pl(px(AVATAR + 8.0))
                .text_sm()
                .text_color(theme.foreground)
                .child(body),
        )
}
