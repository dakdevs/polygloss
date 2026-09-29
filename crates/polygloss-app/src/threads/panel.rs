//! The threads panel (design §8.6, §11.1): every thread of the tab, open
//! ones first, then resolved. Outdated threads are always listed (they may
//! be far from where they were written), and so are threads the diff cannot
//! show: review-level threads and threads whose file or side is not in
//! this diff. A row shows where the thread is, its badges, its author and
//! the first line of its root; clicking it jumps there (the cursor goes to
//! the thread's line) or, for a thread only the panel shows, opens it in
//! place. Hidden agent notes are left out.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use polygloss_core::review::{
    AuthorKind, PositionState, Subject, ThreadKind, ThreadStatus, ThreadView,
};

use super::block::{author_name, excerpt, pill};
use super::placement::ThreadPlace;
use super::{ReviewThreads, activate_thread, block};
use crate::review_tab::ReviewTab;

/// The panel's rows, in order: `(thread id, open)`.
pub fn rows(model: &ReviewThreads) -> Vec<(String, bool)> {
    let listed: Vec<&ThreadView> = model
        .threads()
        .filter(|t| model.shows(t) || outdated(model, t))
        .collect();
    let order = |t: &ThreadView| {
        let place = model.place(&t.id).copied().unwrap_or(ThreadPlace::Panel);
        match (place, &t.anchor.subject) {
            // Review threads first, then the diff's order, then threads
            // this diff cannot show.
            (ThreadPlace::Panel, Subject::Review) => (0, (0, 0, 0)),
            (ThreadPlace::Panel, _) => (2, (0, 0, 0)),
            (p, _) => (1, p.order()),
        }
    };
    let mut open: Vec<&ThreadView> = listed
        .iter()
        .copied()
        .filter(|t| t.status == ThreadStatus::Open)
        .collect();
    let mut resolved: Vec<&ThreadView> = listed
        .iter()
        .copied()
        .filter(|t| t.status == ThreadStatus::Resolved)
        .collect();
    open.sort_by_key(|t| order(t));
    resolved.sort_by_key(|t| order(t));
    open.into_iter()
        .map(|t| (t.id.clone(), true))
        .chain(resolved.into_iter().map(|t| (t.id.clone(), false)))
        .collect()
}

fn outdated(model: &ReviewThreads, t: &ThreadView) -> bool {
    model
        .position(&t.id)
        .is_some_and(|p| p.state == PositionState::Outdated)
}

/// Where a thread is, for its row: `config.rs:12`, `config.rs:12–14`,
/// `greet.ts`, "Review", or the path it was written on when this diff does
/// not show it.
fn location(model: &ReviewThreads, t: &ThreadView) -> (String, Option<String>) {
    let file = |idx: u32| {
        model
            .files()
            .get(idx as usize)
            .map(|f| f.display_path().to_owned())
            .unwrap_or_default()
    };
    let split = |path: &str| match path.rsplit_once('/') {
        Some((dir, name)) => (name.to_owned(), Some(dir.to_owned())),
        None => (path.to_owned(), None),
    };
    match model.place(&t.id).copied().unwrap_or(ThreadPlace::Panel) {
        ThreadPlace::Line {
            file_idx,
            start_line,
            line,
            ..
        } => {
            let (name, dir) = split(&file(file_idx));
            let lines = if start_line < line {
                format!("{}–{}", start_line + 1, line + 1)
            } else {
                (line + 1).to_string()
            };
            (format!("{name}:{lines}"), dir)
        }
        ThreadPlace::File { file_idx } => split(&file(file_idx)),
        ThreadPlace::Panel => match &t.anchor.subject {
            Subject::Review => ("Review".to_owned(), None),
            Subject::File { path } | Subject::Line { path, .. } => {
                let (name, _) = split(path);
                (name, Some("not in this diff".to_owned()))
            }
        },
    }
}

/// The panel of a review tab.
pub fn render(
    model: &Entity<ReviewThreads>,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let (rows, loaded) = {
        let m = model.read(cx);
        (rows(m), m.is_loaded())
    };
    let open = rows.iter().filter(|(_, open)| *open).count();
    let resolved = rows.len() - open;
    let mut list: Vec<AnyElement> = Vec::new();
    let mut resolved_title = false;
    for (id, is_open) in &rows {
        if !is_open && !resolved_title {
            resolved_title = true;
            list.push(section_title(format!("RESOLVED {resolved}"), cx).into_any_element());
        }
        list.push(row(model, id, window, cx));
    }
    let theme = cx.theme();
    let header = h_flex()
        .flex_none()
        .h(px(32.))
        .px_3()
        .gap_2()
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .font_semibold()
        .text_color(theme.muted_foreground)
        .child("THREADS")
        .when(!rows.is_empty(), |el| {
            el.child(div().font_normal().child(match open {
                1 => "1 open".to_owned(),
                n => format!("{n} open"),
            }))
        });
    let body = if rows.is_empty() {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(if loaded {
                "No threads yet"
            } else {
                "Loading threads…"
            })
            .child(div().text_xs().child("Press C on a line to comment."))
            .into_any_element()
    } else {
        div()
            .id("threads-panel-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(list)
            .into_any_element()
    };
    v_flex()
        .debug_selector(|| "threads-panel".into())
        .size_full()
        .bg(theme.sidebar)
        .child(header)
        .child(body)
        .into_any_element()
}

fn section_title(text: String, cx: &Context<ReviewTab>) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .px_3()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_semibold()
        .text_color(theme.muted_foreground)
        .child(text)
}

/// One thread's row (and, for a thread only the panel shows, its card when
/// opened).
fn row(
    model: &Entity<ReviewThreads>,
    id: &str,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let m = model.read(cx);
    let Some(thread) = m.thread_arc(id) else {
        return div().into_any_element();
    };
    let (name, dir) = location(m, &thread);
    let outdated = outdated(m, &thread);
    let panel_only = m.place(id).copied().unwrap_or(ThreadPlace::Panel) == ThreadPlace::Panel;
    let expanded = panel_only && m.is_expanded(id);
    let position = m.position(id).cloned();
    let resolved = thread.status == ThreadStatus::Resolved;
    let draft = thread.draft || thread.comments.iter().any(|c| c.draft);
    let root = thread.comments.first();
    let author = root
        .map(|c| author_name(c.author.kind, &c.author.name))
        .unwrap_or_default();
    let text = root.map(|c| excerpt(&c.body_md)).unwrap_or_default();
    let replies = thread.comments.len().saturating_sub(1);
    let card = expanded.then(|| block::card(model, &thread, position.as_ref(), window, cx));
    let theme = cx.theme();
    let (icon, color) = match (resolved, thread.kind) {
        (true, _) => (IconName::CircleCheck, theme.success),
        (false, ThreadKind::Question) => (IconName::CircleAlert, theme.info),
        (false, ThreadKind::Note) => (IconName::Bot, theme.primary),
        (false, ThreadKind::Comment) if thread.created_by.kind == AuthorKind::Agent => {
            (IconName::Bot, theme.primary)
        }
        (false, ThreadKind::Comment) => (IconName::User, theme.muted_foreground),
    };
    let (sel, q_sel, o_sel, click_id) =
        (id.to_owned(), id.to_owned(), id.to_owned(), id.to_owned());
    v_flex()
        .w_full()
        .border_b_1()
        .border_color(theme.border)
        .child(
            v_flex()
                .id(SharedString::from(format!("threads-panel-{id}")))
                .debug_selector(move || format!("threads-panel-{sel}"))
                .w_full()
                .px_3()
                .py_2()
                .gap_1()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .when(resolved, |el| el.opacity(0.75))
                .on_click(cx.listener(move |tab, _, window, cx| {
                    activate_thread(tab, &click_id, window, cx);
                }))
                .child(
                    h_flex()
                        .w_full()
                        .gap_1p5()
                        .text_xs()
                        .child(Icon::new(icon).xsmall().text_color(color))
                        .child(
                            div()
                                .flex_none()
                                .font_semibold()
                                .text_color(theme.foreground)
                                .child(name),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(dir.unwrap_or_default()),
                        )
                        .when(draft, |el| el.child(pill("Draft", theme.warning)))
                        .when(thread.kind == ThreadKind::Question, |el| {
                            el.child(
                                pill("Question", theme.info).debug_selector(move || {
                                    format!("threads-panel-question-{q_sel}")
                                }),
                            )
                        })
                        .when(thread.kind == ThreadKind::Note, |el| {
                            el.child(pill("Note", theme.muted_foreground))
                        })
                        .when(outdated, |el| {
                            el.child(
                                pill("Outdated", theme.warning).debug_selector(move || {
                                    format!("threads-panel-outdated-{o_sel}")
                                }),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .text_xs()
                        .child(
                            div()
                                .flex_none()
                                .font_medium()
                                .text_color(theme.foreground)
                                .child(author),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(text),
                        ),
                )
                .when(replies > 0, |el| {
                    el.child(div().text_xs().text_color(theme.muted_foreground).child(
                        match replies {
                            1 => "1 reply".to_owned(),
                            n => format!("{n} replies"),
                        },
                    ))
                }),
        )
        .when_some(card, |el, card| el.child(div().px_2().pb_2().child(card)))
        .into_any_element()
}
