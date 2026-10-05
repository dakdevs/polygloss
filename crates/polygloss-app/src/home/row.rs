//! One row of Home (design §11.2): what it shows, computed from a
//! [`ReviewSummary`], and how it is drawn.
//!
//! Each row shows the repo, a title (the review's label, else its branch,
//! else the commit's subject), the kind badge, the status or last verdict,
//! "N / M viewed", open threads and questions, the agent the review is
//! assigned to and the time of the last activity. The pure parts (titles,
//! labels, relative times, [`open_request`] from the review key) are
//! functions of the summary so they can be tested without a window.

use std::path::PathBuf;

use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div, px,
};
use polygloss_core::git::{CompareMode, ReviewKind, Since, Source};
use polygloss_core::review::{OpenRequest, ReviewSummary, Verdict};
use polygloss_core::store::events::Actor;

use crate::review_tab::short_ref;

/// A row's height.
pub const ROW_HEIGHT: f32 = 60.0;
/// A row card's corner radius, the file cards' (ADR-0027).
const CARD_RADIUS: f32 = 8.0;

/// One review on Home, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeRow {
    pub summary: ReviewSummary,
    /// [`title`]: label, branch or commit subject.
    pub title: SharedString,
    /// [`source_line`]: the repo and what the review compares.
    pub source: SharedString,
    /// The client name of the session the review is assigned to
    /// (`claude-code`), if any.
    pub agent: Option<SharedString>,
}

impl HomeRow {
    /// The row of `summary`; `subject` is the commit's subject line (commit
    /// reviews), `agent` the assigned session's client name.
    pub fn new(summary: ReviewSummary, subject: Option<&str>, agent: Option<&str>) -> HomeRow {
        HomeRow {
            title: title(&summary, subject).into(),
            source: source_line(&summary).into(),
            agent: agent.map(|a| SharedString::from(a.to_owned())),
            summary,
        }
    }

    pub fn review_id(&self) -> &str {
        &self.summary.review_id
    }
}

/// A review key taken apart (the grammar of design §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedKey {
    /// `worktree:<worktree>@<branch>#since=<since>`.
    Live {
        worktree: PathBuf,
        branch: String,
        since: Since,
    },
    /// `compare:<base>...<head>` (three-dot) or `compare:<base>..<head>`.
    Compare {
        base: String,
        head: String,
        mode: CompareMode,
    },
    /// `commit:<oid>`.
    Commit { oid: String },
}

/// Parses a review key. Ref names never hold `..`, so the first `...` (else
/// `..`) separates a compare's sides. A live key's worktree path and branch
/// may both hold `@`: the split is the first `@` whose prefix is an existing
/// directory, else the first `@`.
pub fn parse_key(key: &str) -> Option<ParsedKey> {
    if let Some(spec) = key.strip_prefix("compare:") {
        let (base, head, mode) = match spec.split_once("...") {
            Some((b, h)) => (b, h, CompareMode::ThreeDot),
            None => {
                let (b, h) = spec.split_once("..")?;
                (b, h, CompareMode::Direct)
            }
        };
        if base.is_empty() || head.is_empty() {
            return None;
        }
        return Some(ParsedKey::Compare {
            base: base.to_owned(),
            head: head.to_owned(),
            mode,
        });
    }
    if let Some(oid) = key.strip_prefix("commit:") {
        return (!oid.is_empty()).then(|| ParsedKey::Commit {
            oid: oid.to_owned(),
        });
    }
    let rest = key.strip_prefix("worktree:")?;
    let (rest, since) = rest.rsplit_once("#since=")?;
    let since = match since {
        "merge-base" => Since::MergeBase,
        "HEAD" => Since::Head,
        "" => return None,
        oid => Since::Commit(oid.to_owned()),
    };
    let ats: Vec<usize> = rest.match_indices('@').map(|(i, _)| i).collect();
    let at = ats
        .iter()
        .copied()
        .find(|&i| std::path::Path::new(&rest[..i]).is_dir())
        .or_else(|| ats.first().copied())?;
    Some(ParsedKey::Live {
        worktree: PathBuf::from(&rest[..at]),
        branch: rest[at + 1..].to_owned(),
        since,
    })
}

/// The request that opens the review of `summary` again: its source from
/// the key, in its worktree (live) or the repo's main worktree. `None` when
/// the key cannot be parsed.
pub fn open_request(summary: &ReviewSummary) -> Option<OpenRequest> {
    let (worktree, source) = match parse_key(&summary.key)? {
        ParsedKey::Live {
            worktree, since, ..
        } => (worktree, Source::Live { since }),
        ParsedKey::Compare { base, head, mode } => (
            summary.repo_path.clone(),
            Source::Compare { base, head, mode },
        ),
        ParsedKey::Commit { oid } => (summary.repo_path.clone(), Source::Commit { rev: oid }),
    };
    Some(OpenRequest {
        worktree,
        source,
        label: None,
        pin: None,
        actor: Actor::human(),
    })
}

/// The row's title: the label, else the branch (live: the worktree's
/// branch; compare: the head), else the commit's subject (or its short id
/// until the subject is known).
pub fn title(summary: &ReviewSummary, subject: Option<&str>) -> String {
    if let Some(label) = summary.label.as_deref().filter(|l| !l.trim().is_empty()) {
        return label.to_owned();
    }
    match parse_key(&summary.key) {
        Some(ParsedKey::Live { branch, .. }) => branch,
        Some(ParsedKey::Compare { head, .. }) => short_ref(&head).to_owned(),
        Some(ParsedKey::Commit { oid }) => match subject.filter(|s| !s.trim().is_empty()) {
            Some(subject) => subject.trim().to_owned(),
            None => short_oid(&oid).to_owned(),
        },
        None => summary.key.clone(),
    }
}

/// The row's second line: the repo, then what the review compares
/// (`app · main...topic`, `app · 1a2b3c4`, `app · topic · working tree`).
pub fn source_line(summary: &ReviewSummary) -> String {
    let what = match parse_key(&summary.key) {
        Some(ParsedKey::Compare { base, head, mode }) => {
            let sep = match mode {
                CompareMode::ThreeDot => "...",
                CompareMode::Direct => "..",
            };
            format!("{}{sep}{}", short_ref(&base), short_ref(&head))
        }
        Some(ParsedKey::Commit { oid }) => short_oid(&oid).to_owned(),
        Some(ParsedKey::Live { since, .. }) => match since {
            Since::MergeBase => "working tree".to_owned(),
            Since::Head => "working tree since HEAD".to_owned(),
            Since::Commit(oid) => format!("working tree since {}", short_oid(&oid)),
        },
        None => summary.key.clone(),
    };
    let mut parts = vec![summary.repo_display.clone(), what];
    if let Some(ParsedKey::Live { branch, .. }) = parse_key(&summary.key)
        && summary.label.is_some()
    {
        // The title shows the label; the branch still belongs somewhere.
        parts.insert(1, branch);
    }
    parts.join(" · ")
}

fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// How a status pill is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Attention,
    Success,
    Danger,
}

/// The status or last verdict (design §11.2): "Re-review requested",
/// "Changes requested", "Approved", "Commented", else "Open".
pub fn status_label(summary: &ReviewSummary) -> (&'static str, Tone) {
    match summary.status.as_str() {
        "rereview_requested" => ("Re-review requested", Tone::Attention),
        "changes_requested" => ("Changes requested", Tone::Danger),
        "approved" => ("Approved", Tone::Success),
        "commented" => ("Commented", Tone::Neutral),
        _ => match summary.last_submission.as_ref().map(|s| s.verdict) {
            Some(Verdict::RequestChanges) => ("Changes requested", Tone::Danger),
            Some(Verdict::Approve) => ("Approved", Tone::Success),
            Some(Verdict::Comment) => ("Commented", Tone::Neutral),
            None => ("Open", Tone::Neutral),
        },
    }
}

/// The kind badge's text.
pub fn kind_label(kind: ReviewKind) -> &'static str {
    match kind {
        ReviewKind::Live => "LIVE",
        ReviewKind::Compare => "COMPARE",
        ReviewKind::Commit => "COMMIT",
    }
}

/// "N / M viewed", or `None` without files (an unpinned live review).
pub fn viewed_label(summary: &ReviewSummary) -> Option<String> {
    (summary.viewed_total > 0)
        .then(|| format!("{} / {} viewed", summary.viewed_done, summary.viewed_total))
}

/// "1 open thread", "3 open threads", or `None`.
pub fn threads_label(summary: &ReviewSummary) -> Option<String> {
    match summary.open_threads {
        0 => None,
        1 => Some("1 open thread".to_owned()),
        n => Some(format!("{n} open threads")),
    }
}

/// "1 question", "2 questions", or `None`.
pub fn questions_label(summary: &ReviewSummary) -> Option<String> {
    match summary.open_questions {
        0 => None,
        1 => Some("1 question".to_owned()),
        n => Some(format!("{n} questions")),
    }
}

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// When `then_ms` was, seen at `now_ms` (Unix ms): "just now", "5m ago",
/// "3h ago", "yesterday", "4d ago", then the date ("Sep 3", or "Sep 3,
/// 2025" in another year) in the time zone `utc_offset_s` seconds east of
/// UTC. Times in the future (clock skew) read "just now".
pub fn relative_time(now_ms: i64, then_ms: i64, utc_offset_s: i64) -> String {
    let ago = now_ms.saturating_sub(then_ms);
    if ago < MINUTE_MS {
        return "just now".to_owned();
    }
    if ago < HOUR_MS {
        return format!("{}m ago", ago / MINUTE_MS);
    }
    if ago < DAY_MS {
        return format!("{}h ago", ago / HOUR_MS);
    }
    if ago < 2 * DAY_MS {
        return "yesterday".to_owned();
    }
    if ago < 7 * DAY_MS {
        return format!("{}d ago", ago / DAY_MS);
    }
    let local = |ms: i64| civil_from_days((ms / 1000 + utc_offset_s).div_euclid(86_400));
    let (year, month, day) = local(then_ms);
    let (this_year, _, _) = local(now_ms);
    let name = MONTHS[(month - 1) as usize];
    if year == this_year {
        format!("{name} {day}")
    } else {
        format!("{name} {day}, {year}")
    }
}

/// `(year, month 1–12, day 1–31)` of the proleptic Gregorian calendar for
/// `days` since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The local time zone's offset from UTC at `at_ms`, in seconds east.
pub fn local_utc_offset_s(at_ms: i64) -> i64 {
    let t: libc::time_t = (at_ms / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only the `tm` we pass; both live on this
    // stack frame for the call.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff
        }
    }
}

/// Where a row sits for drawing.
pub struct RowPlace {
    pub ix: usize,
    pub selected: bool,
}

/// Draws `row` as a card (the viewport theme's card colors, radius 8): kind
/// badge, title and source, then status, questions, viewed, threads, agent,
/// time and the trailing ⋯ (`actions`).
pub fn render_row(
    row: &HomeRow,
    place: RowPlace,
    now_ms: i64,
    utc_offset_s: i64,
    actions: AnyElement,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let theme = cx.theme();
    let s = &row.summary;
    let (status, tone) = status_label(s);
    let (tone_fg, tone_bg) = tone_colors(tone, cx);
    let meta = |text: String| {
        div()
            .flex_none()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(text)
    };
    let review_id = s.review_id.clone();
    let card = crate::theme::viewport_theme(cx);
    // Hover and selection tint the card (the theme's colors are
    // translucent).
    let (hover, selected) = (
        card.card_background.blend(theme.list_hover),
        card.card_background.blend(theme.list_active),
    );
    h_flex()
        .id(("home-row", place.ix))
        .debug_selector(move || format!("home-row-{review_id}"))
        .h(px(ROW_HEIGHT))
        .w_full()
        .pl_4()
        .pr_2()
        .gap_3()
        .bg(card.card_background)
        .border_1()
        .border_color(card.card_border)
        .rounded(px(CARD_RADIUS))
        .cursor_pointer()
        .hover(|d| d.bg(hover))
        .when(place.selected, |d| d.bg(selected))
        .relative()
        // The keyboard selection: a bar on the left edge, taking no space.
        .when(place.selected, |d| {
            d.child(
                div()
                    .absolute()
                    .left(px(4.))
                    .top(px((ROW_HEIGHT - 28.) / 2.))
                    .w(px(3.))
                    .h(px(28.))
                    .rounded_full()
                    .bg(theme.list_active_border),
            )
        })
        .child(
            div().flex_none().w(px(68.)).child(
                div()
                    .flex()
                    .justify_center()
                    .px_1p5()
                    .py_0p5()
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .text_xs()
                    .font_medium()
                    .text_color(theme.secondary_foreground)
                    .child(kind_label(s.kind)),
            ),
        )
        .child(
            gpui_kit::component::v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .text_color(theme.foreground)
                                .child(row.title.clone()),
                        )
                        .when(s.muted, |d| {
                            d.child(
                                h_flex()
                                    .flex_none()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(
                                        gpui_kit::component::Icon::new(IconName::Bell)
                                            .xsmall()
                                            .text_color(theme.muted_foreground),
                                    )
                                    .child("Muted"),
                            )
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(row.source.clone()),
                ),
        )
        .when_some(questions_label(s), |d, q| {
            d.child(pill(q, theme.warning, theme.warning.opacity(0.14)))
        })
        .child(pill(status.to_owned(), tone_fg, tone_bg))
        .child(
            div()
                .flex_none()
                .w(px(96.))
                .flex()
                .justify_end()
                .when_some(viewed_label(s), |d, v| d.child(meta(v))),
        )
        .child(
            div()
                .flex_none()
                .w(px(104.))
                .flex()
                .justify_end()
                .when_some(threads_label(s), |d, t| d.child(meta(t))),
        )
        .child(
            div()
                .flex_none()
                .w(px(112.))
                .flex()
                .justify_end()
                .when_some(row.agent.clone(), |d, agent| {
                    d.child(
                        h_flex()
                            .gap_1()
                            .px_1p5()
                            .py_0p5()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(
                                gpui_kit::component::Icon::new(IconName::Bot)
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(div().truncate().child(agent)),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .w(px(72.))
                .flex()
                .justify_end()
                .child(meta(relative_time(now_ms, s.updated_at, utc_offset_s))),
        )
        .child(actions)
}

/// A small rounded label.
fn pill(text: String, fg: Hsla, bg: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .text_xs()
        .font_medium()
        .text_color(fg)
        .child(text)
}

fn tone_colors(tone: Tone, cx: &App) -> (Hsla, Hsla) {
    let theme = cx.theme();
    match tone {
        Tone::Neutral => (theme.muted_foreground, theme.muted),
        Tone::Attention => (theme.info, theme.info.opacity(0.14)),
        Tone::Success => (theme.success, theme.success.opacity(0.14)),
        Tone::Danger => (theme.danger, theme.danger.opacity(0.14)),
    }
}
