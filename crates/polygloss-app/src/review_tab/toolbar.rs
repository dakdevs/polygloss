//! The review tab's toolbar (design §11.4), the main column's top row
//! ([`crate::chrome::toolbar_row`]: it moves the window). Left: the repo
//! block, the pills of the review's kind, then the features'
//! ([`crate::features::toolbar_left`]); right: Find, then the features'
//! ([`crate::features::toolbar_right`]): the threads button, `N/M`, the
//! split | unified toggle, the display options menu and Submit review.
//!
//! As the row narrows, things give way in [`Narrow`]'s order. Every frame
//! the row is built at the first step whose natural width fits ([`Fit`],
//! measured by the real layout, searched from the last frame's step);
//! controls read the step with [`narrow`]. Past the last step the left
//! side's items go from its end, the repo block last: a compare review with
//! an iteration pill needs more than the 320 pt main column a 720 pt window
//! may leave.
//!
//! Pills are gpui-kit `Button`s and every other control claims its press,
//! so a press on one never moves the window. Labels a test reads carry the
//! debug selector `"<name>: <text>"` ([`text`]).

use std::path::Path;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, App, AvailableSpace, Bounds, Context, Div, Element, ElementId,
    GlobalElementId, InspectorElementId, InteractiveElement as _, IntoElement, LayoutId,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Style, Styled as _,
    WeakEntity, Window, div, px, relative, size,
};
use polygloss_core::git::ReviewKind;

use crate::keymap::actions::tab as tab_actions;
use crate::review_tab::{ReviewTab, compare_sides, live_branch, repo_dir, repo_name, short_ref};
use crate::tabs::TabItem;

/// How far the toolbar has given way to fit its row, in the order things
/// give way (design §11.4). The threads button, the layout toggle, the
/// display options menu and Submit never hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Narrow {
    /// Everything shows.
    #[default]
    Full,
    /// The repo's parent path hides.
    NoParentPath,
    /// Ref, branch and SHA pills truncate to [`PILL_TEXT_MAX`].
    ShortPills,
    /// The iteration pill shortens ("2/3", "Since review").
    ShortIteration,
    /// Submit review's label becomes "Submit".
    ShortSubmit,
    /// Find moves into the display options menu.
    FindInMenu,
    /// `N/M` moves into it too, as its first row.
    ProgressInMenu,
    /// The kind and iteration pills show only their icons (their text in
    /// the tooltip).
    IconPills,
    /// The repo name truncates to [`REPO_NAME_MAX`].
    ShortRepo,
}

impl Narrow {
    /// Every step, in order.
    pub const ALL: [Narrow; 9] = [
        Narrow::Full,
        Narrow::NoParentPath,
        Narrow::ShortPills,
        Narrow::ShortIteration,
        Narrow::ShortSubmit,
        Narrow::FindInMenu,
        Narrow::ProgressInMenu,
        Narrow::IconPills,
        Narrow::ShortRepo,
    ];
}

/// The longest a ref, branch or SHA pill's text gets once the row is narrow.
pub const PILL_TEXT_MAX: f32 = 64.0;
/// The longest the repo name gets at the last step.
pub const REPO_NAME_MAX: f32 = 48.0;

/// The step the toolbar is drawn at (a [`ReviewTab`] extension).
struct ToolbarNarrow(Narrow);

/// The step the toolbar is drawn at.
pub fn narrow(tab: &ReviewTab) -> Narrow {
    tab.extension::<ToolbarNarrow>()
        .map_or(Narrow::Full, |n| n.0)
}

fn set_narrow(tab: &mut ReviewTab, narrow: Narrow) {
    match tab.extension_mut::<ToolbarNarrow>() {
        Some(step) => step.0 = narrow,
        None => tab.insert_extension(ToolbarNarrow(narrow)),
    }
}

/// The review tab's toolbar row (debug selector `review-toolbar`).
pub(crate) fn render(cx: &mut Context<ReviewTab>) -> AnyElement {
    Fit {
        tab: cx.weak_entity(),
    }
    .into_any_element()
}

/// The toolbar row at step `narrow`, with the first `keep` items of the
/// left side (all of them when `None`), and how many it has in all.
fn build(
    tab: &mut ReviewTab,
    narrow: Narrow,
    keep: Option<usize>,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> (AnyElement, usize) {
    set_narrow(tab, narrow);
    let mut left = crate::features::toolbar_left(tab, window, cx);
    let all = left.len();
    left.truncate(keep.unwrap_or(all));
    let right = crate::features::toolbar_right(tab, window, cx);
    // One group that never reaches the right side.
    let left = h_flex()
        .debug_selector(|| "toolbar-left".into())
        .h_full()
        .min_w_0()
        .overflow_hidden()
        .gap_2()
        .children(left)
        .into_any_element();
    let row = crate::chrome::toolbar_row("review-toolbar", vec![left], right, window, cx);
    (row, all)
}

/// The toolbar's row, 52 pt tall across the main column, built at the first
/// [`Narrow`] step whose natural width fits.
struct Fit {
    tab: WeakEntity<ReviewTab>,
}

impl IntoElement for Fit {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Fit {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(crate::chrome::TOP_ROW_HEIGHT).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let tab = self.tab.upgrade()?;
        let natural = size(
            AvailableSpace::MaxContent,
            AvailableSpace::Definite(bounds.size.height),
        );
        // The row at a step, built by the tab's features (the tab is not
        // being rendered now), whether it fits, and how many items its left
        // side has in all.
        let attempt = |narrow: Narrow, keep: Option<usize>, window: &mut Window, cx: &mut App| {
            let (mut row, all) = tab.update(cx, |tab, cx| build(tab, narrow, keep, window, cx));
            let fits = row.layout_as_root(natural, window, cx).width <= bounds.size.width;
            (row, fits, all)
        };
        // Each step only narrows the row, so the first that fits is found
        // from the last frame's step: back while the step before fits too,
        // else on. While nothing changes that is two builds a frame (more
        // once the left side's items go).
        let mut step = narrow(tab.read(cx)) as usize;
        let mut chosen = None;
        while step > 0 {
            let (row, fits, _) = attempt(Narrow::ALL[step - 1], None, window, cx);
            if !fits {
                break;
            }
            step -= 1;
            chosen = Some(row);
        }
        let mut keep = None;
        let mut row = loop {
            if let Some(row) = chosen.take() {
                break row;
            }
            let (row, fits, all) = attempt(Narrow::ALL[step], keep, window, cx);
            let shown = keep.unwrap_or(all);
            if fits || shown <= 1 {
                break row;
            }
            if step + 1 < Narrow::ALL.len() {
                step += 1;
            } else {
                keep = Some(shown - 1);
            }
        };
        tab.update(cx, |tab, _| set_narrow(tab, Narrow::ALL[step]));
        row.prepaint_as_root(bounds.origin, bounds.size.map(Into::into), window, cx);
        Some(row)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        row: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(row) = row {
            row.paint(window, cx);
        }
    }
}

/// `s`, which tests find as `"<selector>: <s>"`.
pub fn text(selector: &'static str, s: impl Into<SharedString>) -> Div {
    let s: SharedString = s.into();
    let shown = s.clone();
    div()
        .debug_selector(move || format!("{selector}: {s}"))
        .child(shown)
}

/// A tooltip saying `text` (tests find it as `"tooltip: <text>"`).
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| {
        let text = text.clone();
        Tooltip::element(move |_, _| {
            let shown = text.clone();
            let text = text.clone();
            div()
                .debug_selector(move || format!("tooltip: {text}"))
                .child(shown)
        })
        .build(window, cx)
    }
}

/// A capsule (design §11.4): a muted `icon`, then `label` when there is
/// one. A gpui-kit `Button`, so a press on it never moves the window.
pub fn pill(id: &'static str, icon: Lucide, label: Option<Div>, cx: &App) -> Button {
    Button::new(id)
        .debug_selector(move || id.into())
        .secondary()
        .small()
        .rounded(px(12.))
        .icon(Icon::new(icon).text_color(cx.theme().muted_foreground))
        .when_some(label, |pill, label| pill.child(label))
}

/// A pill's text (found as `"<selector>: <text>"`), truncating when its
/// pill is capped.
pub fn pill_text(selector: &'static str, s: impl Into<SharedString>) -> Div {
    text(selector, s).min_w_0().truncate()
}

/// The repo's parent directory as the toolbar shows it: `~` for `home`
/// and below it (whole path components only), else in full.
pub fn parent_path(dir: &Path, home: Option<&Path>) -> String {
    let parent = dir.parent().unwrap_or(dir);
    match home.and_then(|home| parent.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => parent.display().to_string(),
    }
}

/// The repo block: its name (bold) over its parent directory (dim); its
/// tooltip names the review's kind and the repo's path. Not a control: it
/// moves the window like the row.
pub fn repo_block(tab: &ReviewTab, cx: &App) -> AnyElement {
    let narrow = narrow(tab);
    let theme = cx.theme();
    let dir = repo_dir(&tab.opened);
    let parent = parent_path(&dir, std::env::home_dir().as_deref());
    let tip = format!("{} review · {}", tab.opened.kind.as_str(), dir.display());
    v_flex()
        .id("repo-block")
        .debug_selector(|| "repo-block".into())
        .min_w_0()
        .when(narrow >= Narrow::ShortRepo, |block| {
            block.flex_none().max_w(px(REPO_NAME_MAX))
        })
        .child(
            text("repo-name", repo_name(&tab.opened))
                .truncate()
                .text_sm()
                .font_semibold()
                .line_height(relative(1.2))
                .text_color(theme.foreground),
        )
        .when(narrow < Narrow::NoParentPath, |block| {
            block.child(
                text("repo-parent", parent)
                    .truncate()
                    .text_xs()
                    .line_height(relative(1.2))
                    .text_color(theme.muted_foreground),
            )
        })
        .tooltip(tooltip(tip))
        .into_any_element()
}

/// The review's label (`PR #123`), as Home last listed it.
fn review_label(review_id: &str, cx: &App) -> Option<String> {
    let (_, main) = crate::window::main_window(cx)?;
    let TabItem::Home(home) = main.read(cx).tabs().get(0)? else {
        return None;
    };
    let home = home.read(cx);
    let row = home.rows().iter().find(|r| r.review_id() == review_id)?;
    row.summary.label.clone()
}

/// The pills of the review's kind (design §11.4): a commit's short SHA; a
/// compare's base and head refs around `…` (three-dot) or `..` (direct);
/// a live review's branch (its Live pill is `live::toolbar_left`'s).
pub fn kind_pills(tab: &ReviewTab, cx: &App) -> Vec<AnyElement> {
    let narrow = narrow(tab);
    let icons_only = narrow >= Narrow::IconPills;
    let opened = &tab.opened;
    let theme = cx.theme();
    // Ref, branch and SHA pills: capped from `ShortPills`, icons only from
    // `IconPills`.
    let label = |selector: &'static str, s: String| {
        (!icons_only).then(|| {
            pill_text(selector, s)
                .when(narrow >= Narrow::ShortPills, |t| t.max_w(px(PILL_TEXT_MAX)))
        })
    };
    match opened.kind {
        ReviewKind::Commit => {
            let Some(head) = &opened.head_commit else {
                return Vec::new();
            };
            let sha = label("commit-pill-label", head.short().to_owned()).map(|t| {
                t.font_family(theme.mono_font_family.clone())
                    .text_color(crate::theme::viewport_theme(cx).commit_sha)
            });
            vec![
                pill("commit-pill", Lucide::GitCommitHorizontal, sha, cx)
                    .tooltip(format!("Commit {head}"))
                    .into_any_element(),
            ]
        }
        ReviewKind::Compare => {
            let (sep, base, head) = compare_sides(&opened.review_key);
            let mark = if sep == "..." { "\u{2026}" } else { ".." };
            let prefix = review_label(&opened.review_id, cx)
                .map(|l| format!("{l} · "))
                .unwrap_or_default();
            let side = |id: &'static str, selector, side: &str, r: &str| {
                pill(
                    id,
                    Lucide::GitBranch,
                    label(selector, short_ref(r).to_owned()),
                    cx,
                )
                .tooltip(format!("{prefix}{side}: {r}"))
                .into_any_element()
            };
            vec![
                side("ref-pill-base", "ref-pill-base-label", "Base", base),
                text("compare-mode", mark)
                    .flex_none()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                side("ref-pill-head", "ref-pill-head-label", "Head", head),
            ]
        }
        ReviewKind::Live => {
            let branch = live_branch(opened).unwrap_or("worktree").to_owned();
            vec![
                pill(
                    "branch-pill",
                    Lucide::GitBranch,
                    label("branch-pill-label", branch.clone()),
                    cx,
                )
                .tooltip(format!("Branch {branch}"))
                .into_any_element(),
            ]
        }
    }
}

/// Find (⌘F), until the row is narrow enough to move it into the display
/// options menu ([`Narrow::FindInMenu`]).
pub fn find_button(tab: &ReviewTab) -> Option<AnyElement> {
    if narrow(tab) >= Narrow::FindInMenu {
        return None;
    }
    // Sent through the diff's focus: it reaches the tab wherever the
    // keyboard is.
    let focus = tab.viewport_focus().clone();
    Some(
        Button::new("toolbar-find")
            .debug_selector(|| "toolbar-find".into())
            .icon(IconName::Search)
            .ghost()
            .small()
            .tooltip_with_action("Find in all files", &tab_actions::Find, Some("Tab"))
            .on_click(move |_, window, cx| focus.dispatch_action(&tab_actions::Find, window, cx))
            .into_any_element(),
    )
}
