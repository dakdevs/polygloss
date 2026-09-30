//! The threads panel (design §8.6, §11.1): every thread of the tab, open
//! ones first, then resolved. Outdated threads are always listed (they may
//! be far from where they were written), and so are threads the diff cannot
//! show: review-level threads and threads whose file or side is not in
//! this diff. A row shows where the thread is, its badges, its author and
//! the first line of its root; clicking it jumps there (the cursor goes to
//! the thread's line) or, for a thread only the panel shows, opens it in
//! place. Hidden agent notes are left out unless outdated (then they open
//! under their row, since the diff does not show them).
//!
//! Counts: the header's "n open" counts open threads that wait on someone,
//! like the file headers' and the tree's badges (agent notes are FYI: they
//! are counted apart, "· k notes"); review-level threads and threads this
//! diff cannot show count here but have no file badge. `.` / `,` visit
//! every open thread the diff shows, notes included.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use polygloss_core::review::{
    AuthorKind, PositionState, Subject, ThreadKind, ThreadStatus, ThreadView,
};
use polygloss_diff::Side;

use super::block::{author_name, excerpt, pill};
use super::placement::ThreadPlace;
use super::{DiffOrderer, ReviewThreads, activate_thread, block};
use crate::review_tab::ReviewTab;

/// The panel's rows, in order: `(thread id, open)`. Threads in the diff
/// are in its top-to-bottom order (old and new sides interleaved as the
/// viewport shows them).
pub fn rows(model: &ReviewThreads, cx: &App) -> Vec<(String, bool)> {
    let listed: Vec<&ThreadView> = model
        .threads()
        .filter(|t| model.shows(t) || outdated(model, t))
        .collect();
    let mut orderer = DiffOrderer::new(model, model.viewport.read(cx));
    let mut order = |t: &ThreadView| {
        let place = model.place(&t.id).copied().unwrap_or(ThreadPlace::Panel);
        let none = (0, 0, 0, 0, 0);
        match (place, &t.anchor.subject) {
            // Review threads first, then the diff's order, then threads
            // this diff cannot show.
            (ThreadPlace::Panel, Subject::Review) => (0, none),
            (ThreadPlace::Panel, _) => (2, none),
            (p, _) => (1, orderer.place(&p)),
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
    open.sort_by_cached_key(|t| order(t));
    resolved.sort_by_cached_key(|t| order(t));
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
/// not show it. Old-side lines read GitHub's way, `config.rs:L12` (left),
/// so they never look like the new line with the same number.
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
            side,
            start_line,
            line,
        } => {
            let (name, dir) = split(&file(file_idx));
            let l = match side {
                Side::Old => "L",
                Side::New => "",
            };
            let lines = if start_line < line {
                format!("{l}{}–{l}{}", start_line + 1, line + 1)
            } else {
                format!("{l}{}", line + 1)
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
    let (rows, loaded, notes, composer, focus, scroll, selected) = {
        let m = model.read(cx);
        let rows = rows(m, cx);
        let notes = rows
            .iter()
            .filter(|(id, open)| *open && m.thread(id).is_some_and(|t| t.kind == ThreadKind::Note))
            .count();
        // The review-level composer (T3.10) sits at the top.
        let composer = crate::composer::review_composer(m, cx);
        (
            rows,
            m.is_loaded(),
            notes,
            composer,
            m.panel_focus().clone(),
            m.panel_scroll().clone(),
            m.selected().map(str::to_owned),
        )
    };
    // The selected row is marked while the panel has the keyboard (and
    // shaded otherwise, so the keys' target is never a guess).
    let focused = focus.is_focused(window) && window.last_input_was_keyboard();
    let listed_open = rows.iter().filter(|(_, open)| *open).count();
    let open = listed_open - notes;
    let resolved = rows.len() - listed_open;
    let mut list: Vec<AnyElement> = Vec::new();
    let mut resolved_title = false;
    for (id, is_open) in &rows {
        if !is_open && !resolved_title {
            resolved_title = true;
            list.push(section_title(format!("RESOLVED {resolved}"), cx).into_any_element());
        }
        let is_selected = selected.as_deref() == Some(id.as_str());
        list.push(row(model, id, is_selected, focused, window, cx));
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
            let mut text = format!("{open} open");
            match notes {
                0 => {}
                1 => text.push_str(" · 1 note"),
                n => text.push_str(&format!(" · {n} notes")),
            }
            el.child(
                div()
                    .debug_selector(|| "threads-panel-count".into())
                    .font_normal()
                    .child(text),
            )
        })
        .child(div().flex_1())
        .child(
            Button::new("comment-on-review")
                .debug_selector(|| "comment-on-review".into())
                .xsmall()
                .ghost()
                .icon(IconName::Plus)
                .label("Comment on review")
                .tooltip("Start a review-level comment (a draft until you submit)")
                .on_click(cx.listener(|tab, _, window, cx| {
                    crate::composer::open_review_composer(tab, window, cx)
                })),
        );
    let body = if rows.is_empty() && composer.is_some() {
        div().flex_1().into_any_element()
    } else if rows.is_empty() {
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
            .track_scroll(&scroll)
            .children(list)
            .into_any_element()
    };
    v_flex()
        .debug_selector(|| "threads-panel".into())
        .key_context("ThreadsPanel")
        .track_focus(&focus)
        .size_full()
        .bg(theme.sidebar)
        .child(header)
        .when_some(composer, |el, c| el.child(c))
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
    selected: bool,
    focused: bool,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> AnyElement {
    let m = model.read(cx);
    let Some(thread) = m.thread_arc(id) else {
        return div().into_any_element();
    };
    let (name, dir) = location(m, &thread);
    let outdated = outdated(m, &thread);
    // Threads the diff does not show (panel-only, hidden notes) open here.
    let panel_only = m.place(id).copied().unwrap_or(ThreadPlace::Panel) == ThreadPlace::Panel
        || !m.shows(&thread);
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
    let (sel, q_sel, o_sel, click_id, mark_sel) = (
        id.to_owned(),
        id.to_owned(),
        id.to_owned(),
        id.to_owned(),
        id.to_owned(),
    );
    v_flex()
        .w_full()
        .border_b_1()
        .border_color(theme.border)
        .child(
            v_flex()
                .id(SharedString::from(format!("threads-panel-{id}")))
                .debug_selector(move || format!("threads-panel-{sel}"))
                .relative()
                .w_full()
                .px_3()
                .py_2()
                .gap_1()
                .cursor_pointer()
                .hover(|s| s.bg(theme.list_hover))
                .when(selected, |el| {
                    el.bg(theme.list_active).child(
                        div()
                            .debug_selector(move || format!("threads-panel-selected-{mark_sel}"))
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(if focused { 3. } else { 2. }))
                            .bg(if focused {
                                theme.ring
                            } else {
                                theme.muted_foreground.opacity(0.5)
                            }),
                    )
                })
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
