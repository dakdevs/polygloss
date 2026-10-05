//! The header card (design §11.6, ADR-0027): the review's own card above the
//! first file card, the viewport's prelude, scrolling with the diff.
//!
//! - **Commit:** the author's avatar (an initial on a color from FNV-1a of the
//!   lowercased email, [`avatar_color`]; never fetched), the subject, "<author>
//!   committed <time>", and the short SHA.
//! - **Compare:** the label or `base…head`, "N commits · <head author>
//!   committed <time>", and "Show commits" (newest first; 50, then "and N
//!   more").
//! - **Live:** "Changes on <branch>" ("Uncommitted changes on <branch>" when
//!   the base is HEAD), "vs <base>", and Snapshot (disabled once pinned).
//!
//! Every kind shows "N files · +X −Y" in its right cluster, before the SHA or
//! Snapshot. Times are relative to the app clock ([`set_clock`]).
//!
//! The card's facts come from git on the background executor ([`reload`]);
//! the card then becomes the prelude. Whatever changes its height (the commit
//! list, a reload) sets the prelude again, so the viewport measures it again
//! even off screen; the document's top anchor keeps a review at the top of
//! the card while it lands or grows.
//!
//! Debug selectors: `header-card`, `header-avatar`, `header-title`,
//! `header-byline`, `header-stats`, `header-sha`, `header-commits-toggle`,
//! `header-commits` (the list), `header-commit-<i>`, `header-commits-more`
//! and `live-snapshot`; the texts as `"<name>: <text>"`.

use std::rc::Rc;

use anyhow::Context as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Global, Hsla, InteractiveElement as _,
    IntoElement as _, ParentElement as _, SharedString, Styled as _, Task, WeakEntity, Window, div,
    px,
};
use polygloss_core::git::listing::{CommitDetails, commit_details, range_commits};
use polygloss_core::git::{Git, ReviewKind, Since};
use polygloss_core::review::{Core, OpenedDiff};
use polygloss_core::store::events::now_ms;
use polygloss_highlight::{Appearance, Rgba};
use polygloss_viewport::{RenderBlock, group_digits};

use crate::app_state::AppState;
use crate::home::row::{local_utc_offset_s, relative_time};
use crate::review_tab::toolbar::text;
use crate::review_tab::{ReviewTab, compare_sides, live_branch, short_ref};

/// Commits a compare's card lists at most.
const MAX_COMMITS: u32 = 50;
/// The card's avatar; the commit list's are smaller.
const AVATAR: f32 = 26.0;
const ROW_AVATAR: f32 = 18.0;
const ROW_HEIGHT: f32 = 28.0;

/// Avatar fills for themes with fewer than eight players (Pierre), per
/// appearance (research: redesign reference).
const LIGHT_HUES: [u32; 8] = [
    0xb25e7d, 0x4f74b8, 0x3f8f62, 0xb07a3a, 0x7a5cc0, 0x2f8a9c, 0xb8634f, 0x6b7280,
];
const DARK_HUES: [u32; 8] = [
    0x4c8dff, 0x7fa2e6, 0x6cc08f, 0xd9a465, 0xa68be6, 0x5cc1d6, 0xe08b77, 0x9ca3af,
];

/// The avatar fill for `email`: FNV-1a (32-bit) of the lowercased email,
/// mod 8, into the theme's first eight `players`, or into fixed hues of
/// `appearance` when the theme has fewer.
pub fn avatar_color(email: &str, players: &[Hsla], appearance: Appearance) -> Hsla {
    let hash = email.to_lowercase().bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    let i = (hash % 8) as usize;
    if players.len() >= 8 {
        return players[i];
    }
    let hues = match appearance {
        Appearance::Light => LIGHT_HUES,
        Appearance::Dark => DARK_HUES,
    };
    gpui_kit::rgb(hues[i]).into()
}

/// The avatar's letter: the first alphanumeric character of `name`,
/// uppercased; `?` when it has none.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// What the card reads from git.
enum Content {
    Commit(CommitDetails),
    Compare {
        label: Option<String>,
        head: CommitDetails,
        /// The newest [`MAX_COMMITS`] of `base..head`, and how many it has.
        commits: Vec<CommitDetails>,
        total: u32,
    },
    Live,
}

/// The header card of a review tab (a [`ReviewTab`] extension).
pub struct HeaderCard {
    /// `None` until the first load lands (or after one failed): no card.
    content: Option<Content>,
    commits_open: bool,
    generation: u64,
    _load: Option<Task<()>>,
}

/// The clock relative times are read on (Unix ms), a GPUI global: the
/// system's unless pinned.
struct Clock(Rc<dyn Fn() -> i64>);

impl Global for Clock {}

/// Reads relative times on `clock` (Unix ms) from now on (tests and
/// screenshots pin it).
pub fn set_clock(clock: impl Fn() -> i64 + 'static, cx: &mut App) {
    cx.set_global(Clock(Rc::new(clock)));
}

fn now(cx: &App) -> i64 {
    cx.try_global::<Clock>().map_or_else(now_ms, |c| (c.0)())
}

/// Gives a new tab its header card: loaded in the background, then shown.
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    tab.insert_extension(HeaderCard {
        content: None,
        commits_open: false,
        generation: 0,
        _load: None,
    });
    reload(tab, cx);
}

/// Reads the card of what the tab shows again (a refresh or an iteration
/// switch changed it); the card shown stays until the new one lands.
pub fn reload(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let opened = tab.opened.clone();
    let core = AppState::global(cx).core.clone();
    let Some(card) = tab.extension_mut::<HeaderCard>() else {
        return;
    };
    card.generation += 1;
    let generation = card.generation;
    card._load = Some(cx.spawn(async move |tab, cx| {
        let loaded = cx
            .background_spawn(async move { load(&core, &opened) })
            .await;
        tab.update(cx, |tab, cx| landed(tab, generation, loaded, cx))
            .ok();
    }));
}

/// Git reads only (offline, design §6.2).
fn load(core: &Core, opened: &OpenedDiff) -> anyhow::Result<Content> {
    if opened.kind == ReviewKind::Live {
        return Ok(Content::Live);
    }
    // A bare repo has no worktree: git runs in its git dir.
    let repo = &opened.repo;
    let git = Git::new(
        repo.toplevel
            .clone()
            .unwrap_or_else(|| repo.git_dir.clone()),
    );
    let head = opened
        .head_commit
        .as_ref()
        .context("a commit or compare review has a head commit")?;
    if opened.kind == ReviewKind::Commit {
        return Ok(Content::Commit(commit_details(&git, head)?));
    }
    let label = core
        .review_summary(&opened.review_id)?
        .and_then(|s| s.label);
    let (commits, total) = match &opened.base.commit {
        Some(base) => range_commits(&git, base, head, MAX_COMMITS)?,
        None => (Vec::new(), 0),
    };
    Ok(Content::Compare {
        label,
        head: commit_details(&git, head)?,
        commits,
        total,
    })
}

fn landed(
    tab: &mut ReviewTab,
    generation: u64,
    loaded: anyhow::Result<Content>,
    cx: &mut Context<ReviewTab>,
) {
    let key = &tab.opened.review_key;
    let content = loaded
        .inspect_err(|e| tracing::warn!("the header card of {key}: {e:#}"))
        .ok();
    let Some(card) = tab.extension_mut::<HeaderCard>() else {
        return;
    };
    if card.generation != generation {
        return;
    }
    card.content = content;
    card._load = None;
    show(tab, cx);
}

/// Sets the card as the viewport's prelude (measured again), or removes it
/// while there is nothing to show.
fn show(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    let shown = tab
        .extension::<HeaderCard>()
        .is_some_and(|c| c.content.is_some());
    let weak = cx.weak_entity();
    let prelude = shown.then(|| -> RenderBlock { Rc::new(move |_, cx| render(&weak, cx)) });
    tab.viewport.update(cx, |v, cx| v.set_prelude(prelude, cx));
}

/// "Show commits" ⇄ "Hide commits": the card's height changes.
fn toggle_commits(tab: &mut ReviewTab, cx: &mut Context<ReviewTab>) {
    if let Some(card) = tab.extension_mut::<HeaderCard>() {
        card.commits_open = !card.commits_open;
    }
    show(tab, cx);
}

/// `s` in an element found as `name` (bounds) holding `"<name>: <s>"`.
fn labeled(name: &'static str, s: impl Into<SharedString>) -> Div {
    div()
        .debug_selector(move || name.into())
        .min_w_0()
        .child(text(name, s).truncate())
}

/// The card, as the prelude renders it each frame it shows.
fn render(tab: &WeakEntity<ReviewTab>, cx: &App) -> AnyElement {
    let Some(entity) = tab.upgrade() else {
        return div().into_any_element();
    };
    let t = entity.read(cx);
    let Some((content, commits_open)) = t
        .extension::<HeaderCard>()
        .and_then(|card| Some((card.content.as_ref()?, card.commits_open)))
    else {
        return div().into_any_element();
    };
    let theme = cx.theme();
    let colors = crate::theme::viewport_theme(cx);
    let look = Look::new(cx);
    let parts = match content {
        Content::Commit(c) => Parts {
            avatar: Some(avatar(c, AVATAR, Some("header-avatar"), &look)),
            title: c.subject.clone(),
            byline: format!("{} committed {}", c.author_name, look.ago(c)),
            toggle: None,
            trailing: Some(
                labeled("header-sha", c.oid.short().to_string())
                    .flex_none()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(colors.commit_sha)
                    .into_any_element(),
            ),
            list: None,
        },
        Content::Compare {
            label,
            head,
            commits,
            total,
        } => {
            let title = label.clone().unwrap_or_else(|| {
                let (sep, base, head) = compare_sides(&t.opened.review_key);
                let mark = if sep == "..." { "\u{2026}" } else { ".." };
                format!("{}{mark}{}", short_ref(base), short_ref(head))
            });
            let by = format!("{} committed {}", head.author_name, look.ago(head));
            let byline = if t.opened.base.commit.is_some() {
                format!("{} · {by}", plural(u64::from(*total), "commit"))
            } else {
                by
            };
            Parts {
                avatar: None,
                title,
                byline,
                toggle: (*total > 0).then(|| commits_toggle(commits_open, tab.clone())),
                trailing: None,
                // Open with no commits (after a reload): no toggle, no list.
                list: (commits_open && *total > 0).then(|| commit_list(commits, *total, &look, cx)),
            }
        }
        Content::Live => {
            let branch = live_branch(&t.opened).unwrap_or("HEAD");
            let title = match crate::live::since(t) {
                Some(Since::Head) => format!("Uncommitted changes on {branch}"),
                _ => format!("Changes on {branch}"),
            };
            Parts {
                avatar: None,
                title,
                byline: format!("vs {}", crate::iterations::base_name(&t.opened)),
                toggle: None,
                trailing: Some(snapshot(t, tab.clone()).into_any_element()),
                list: None,
            }
        }
    };
    // The reference's proportions: a 15 pt subject over a 13 pt line, about
    // 56 pt in all.
    let top = h_flex()
        .gap_3()
        .px_4()
        .py_2p5()
        .children(parts.avatar)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    labeled("header-title", parts.title)
                        .text_size(px(15.))
                        .line_height(px(18.))
                        .font_semibold(),
                )
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_2()
                        .text_size(px(13.))
                        .line_height(px(17.))
                        .text_color(theme.muted_foreground)
                        .child(labeled("header-byline", parts.byline))
                        .children(parts.toggle),
                ),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_4()
                .text_size(px(13.))
                .child(labeled("header-stats", stats(t, cx)).text_color(theme.muted_foreground))
                .children(parts.trailing),
        );
    v_flex()
        .debug_selector(|| "header-card".into())
        .w_full()
        .overflow_hidden()
        .bg(colors.card_background)
        .border_1()
        .border_color(colors.card_border)
        .rounded(px(8.))
        .text_color(theme.foreground)
        .child(top)
        .children(parts.list)
        .into_any_element()
}

/// The pieces that differ by kind: left to right, then below.
struct Parts {
    avatar: Option<Div>,
    title: String,
    byline: String,
    /// After the byline: "Show commits".
    toggle: Option<Button>,
    /// After the stats: the SHA or Snapshot.
    trailing: Option<AnyElement>,
    /// Below: the commit list.
    list: Option<Div>,
}

/// "Show commits" ⇄ "Hide commits".
fn commits_toggle(open: bool, tab: WeakEntity<ReviewTab>) -> Button {
    let (icon, label) = if open {
        (IconName::ChevronDown, "Hide commits")
    } else {
        (IconName::ChevronRight, "Show commits")
    };
    Button::new("header-commits-toggle")
        .debug_selector(|| "header-commits-toggle".into())
        .icon(icon)
        .label(label)
        .ghost()
        .xsmall()
        .on_click(move |_, _, cx| {
            tab.update(cx, toggle_commits).ok();
        })
}

/// "1 file", "1,204 files".
fn plural(n: u64, noun: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{} {noun}{s}", group_digits(n))
}

/// "N files · +X −Y" over every file of the diff, "N files · …" until each
/// is counted. (T6.15 narrows it to the uncategorized files.)
fn stats(tab: &ReviewTab, cx: &App) -> String {
    let viewport = tab.viewport.read(cx);
    let n = viewport.document().files().len() as u32;
    let files = plural(u64::from(n), "file");
    match crate::tree::footer::totals(0..n, viewport) {
        Some((added, removed)) => format!(
            "{files} · +{} −{}",
            group_digits(added),
            group_digits(removed)
        ),
        None => format!("{files} · …"),
    }
}

/// A live card's Snapshot: pins the state shown as an iteration (design
/// §5.2); disabled once it is pinned.
fn snapshot(tab: &ReviewTab, weak: WeakEntity<ReviewTab>) -> Button {
    let current = crate::iterations::current(tab);
    let pinned = current
        .iteration
        .as_ref()
        .filter(|it| it.diff_id == current.diff_id)
        .map(|it| it.seq);
    Button::new("live-snapshot")
        .debug_selector(|| "live-snapshot".into())
        .icon(Icon::new(Lucide::Camera))
        .label("Snapshot")
        .outline()
        .small()
        .disabled(pinned.is_some())
        .tooltip(match pinned {
            Some(seq) => format!("Saved as iteration {seq}"),
            None => "Pin this state as an iteration".to_owned(),
        })
        .on_click(move |_, window, cx| {
            weak.update(cx, |tab, cx| crate::live::snapshot(tab, window, cx))
                .ok();
        })
}

/// What the card's parts read once per frame: the time, and the theme's
/// avatar colors.
struct Look {
    now_ms: i64,
    utc_offset_s: i64,
    /// The active theme's player colors (Zed's `players[].cursor`).
    players: Vec<Hsla>,
    appearance: Appearance,
}

impl Look {
    fn new(cx: &App) -> Look {
        let now_ms = now(cx);
        let players = cx
            .try_global::<crate::theme::ActiveTheme>()
            .and_then(|active| active.zed.style.get("players")?.as_array())
            .into_iter()
            .flatten()
            .filter_map(|p| p.get("cursor")?.as_str().and_then(Rgba::parse))
            .map(|c| gpui_kit::rgba(c.to_u32()).into())
            .collect();
        Look {
            now_ms,
            utc_offset_s: local_utc_offset_s(now_ms),
            players,
            appearance: crate::theme::viewport_theme(cx).appearance,
        }
    }

    /// When `c` was committed: "56m ago", "Jan 2".
    fn ago(&self, c: &CommitDetails) -> String {
        relative_time(self.now_ms, c.committed_at * 1000, self.utc_offset_s)
    }
}

/// The author's initial on their color; found as `name` (and the initial as
/// `"<name>: <initial>"`) when given one.
fn avatar(c: &CommitDetails, size: f32, name: Option<&'static str>, look: &Look) -> Div {
    let fill = avatar_color(&c.author_email, &look.players, look.appearance);
    // Dark text on the light fills of dark themes.
    let ink = if fill.l > 0.6 {
        gpui_kit::black()
    } else {
        gpui_kit::white()
    };
    let letter = initial(&c.author_name);
    let letter = match name {
        Some(name) => text(name, letter),
        None => div().child(letter),
    };
    div()
        .when_some(name, |d, name| d.debug_selector(move || name.into()))
        .flex_none()
        .size(px(size))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(fill)
        .text_color(ink)
        .text_size(px(size * 0.5))
        .font_semibold()
        .child(letter)
}

/// A compare's commits, newest first, then "and N more".
fn commit_list(commits: &[CommitDetails], total: u32, look: &Look, cx: &App) -> Div {
    let theme = cx.theme();
    let colors = crate::theme::viewport_theme(cx);
    let more = u64::from(total).saturating_sub(commits.len() as u64);
    v_flex()
        .debug_selector(|| "header-commits".into())
        .py_1()
        .border_t_1()
        .border_color(colors.card_border)
        .text_size(px(13.))
        .children(commits.iter().enumerate().map(|(i, c)| {
            h_flex()
                .debug_selector(move || format!("header-commit-{i}"))
                .h(px(ROW_HEIGHT))
                .px_4()
                .gap_2()
                .child(avatar(c, ROW_AVATAR, None, look).text_size(px(10.)))
                .child(
                    text("header-commit", c.subject.clone())
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(format!("{} · {}", c.author_name, look.ago(c))),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(colors.commit_sha)
                        .child(c.oid.short().to_string()),
                )
        }))
        .when(more > 0, |list| {
            list.child(
                text(
                    "header-commits-more",
                    format!("and {} more", group_digits(more)),
                )
                .px_4()
                .py_1()
                .text_color(theme.muted_foreground),
            )
        })
}
