//! Home's two dialogs: the confirmation before pruning a review, and the
//! "Assign to session…" picker (OQ-32).

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, InteractiveElement as _, ParentElement as _, SharedString, Styled as _, WeakEntity,
    Window, div, px,
};
use polygloss_core::review::SessionInfo;

use crate::home::HomeView;
use crate::space::{TextStyleExt as _, gap, layout, text};

/// Asks before pruning `review_id` (`title` in `repo`); OK prunes it.
pub(crate) fn confirm_prune(
    home: WeakEntity<HomeView>,
    review_id: String,
    title: SharedString,
    repo: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (home, review_id) = (home.clone(), review_id.clone());
        alert
            .title("Prune this review?")
            .description(format!(
                "“{title}” in {repo} and its iterations, threads, comments and \
                 drafts are deleted. This cannot be undone."
            ))
            .ok_text("Prune")
            .ok_variant(ButtonVariant::Danger)
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                home.update(cx, |home, cx| home.prune(review_id.clone(), window, cx))
                    .ok();
                true
            })
    });
}

/// The session picker for `review_id`: every agent session seen in the last
/// 7 days, the current assignee checked; choosing one assigns the review.
pub(crate) fn pick_session(
    home: WeakEntity<HomeView>,
    review_id: String,
    title: SharedString,
    current: Option<String>,
    sessions: Vec<SessionInfo>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        // Rows touch, as in every dense list (ADR-0031).
        let list = v_flex().children(sessions.iter().enumerate().map(|(i, s)| {
            let (home, review_id, session_id) = (home.clone(), review_id.clone(), s.id.clone());
            let assigned = current.as_deref() == Some(s.id.as_str());
            let place = s
                .cwd
                .as_ref()
                .map(|p| format!(" · {}", tildify(p)))
                .unwrap_or_default();
            let label = format!("{}{place}", s.client_name);
            Button::new(("assign-session", i))
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .when(assigned, |b| b.icon(IconName::Check))
                .when(!assigned, |b| b.icon(IconName::Bot))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_between()
                        .gap(px(gap::GROUP))
                        .child(div().truncate().child(label))
                        .child(
                            div()
                                .flex_none()
                                .text_style(text::SMALL)
                                .text_color(theme.muted_foreground)
                                .child(short_session(&s.id)),
                        ),
                )
                .debug_selector(move || format!("assign-session-{i}"))
                .on_click(move |_, window, cx| {
                    home.update(cx, |home, cx| {
                        home.assign(review_id.clone(), session_id.clone(), window, cx)
                    })
                    .ok();
                    window.close_dialog(cx);
                })
        }));
        dialog
            .title(format!("Assign “{title}” to a session"))
            .w(px(layout::DIALOG_W))
            .child(
                v_flex()
                    .gap(px(gap::GROUP))
                    .child(
                        div()
                            .text_style(text::BODY)
                            .text_color(theme.muted_foreground)
                            .child("The agent session gets this review's submissions."),
                    )
                    .when(sessions.is_empty(), |d| {
                        d.child(
                            div()
                                .py(px(gap::SECTION))
                                .text_style(text::UI)
                                .text_color(theme.muted_foreground)
                                .child("No agent sessions in the last 7 days."),
                        )
                    })
                    .child(list),
            )
    });
}

/// `path` with the home directory as `~`.
fn tildify(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

/// A session id short enough for a list (`pg-` ids are long UUIDs).
fn short_session(id: &str) -> String {
    let n = id.chars().count();
    if n <= 12 {
        return id.to_owned();
    }
    let tail: String = id.chars().skip(n - 8).collect();
    format!("…{tail}")
}
