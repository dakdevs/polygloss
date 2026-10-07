//! The totals of a partitioned review (design §11.5, §11.15): the tree's
//! footer and the header card count the files of the main list ([`Totals`])
//! and add one [`Chip`] per category section ("6 tests"); their tooltip is
//! the [`Breakdown`] without, with and of only the categorized files.

use std::sync::Arc;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, App, Entity, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, Styled as _, Window, div, px,
};
use polygloss_core::categories::CategoryId;
use polygloss_diff::{FileChange, FileKind};
use polygloss_viewport::{DiffViewport, group_digits};

use super::Partition;
use crate::space::{TextStyleExt as _, gap, pad, text};

/// A category's chip in the totals (design §11.15): "1 test", "6 tests".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub category: CategoryId,
    pub text: SharedString,
}

/// One chip per section of `partition`, in its order.
pub fn chips(partition: &Partition) -> Vec<Chip> {
    partition
        .sections
        .iter()
        .map(|s| Chip {
            category: s.id.clone(),
            text: s.info.chip(s.files.len()).into(),
        })
        .collect()
}

/// Files and their lines over some of a diff's files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u32,
    pub additions: u64,
    pub deletions: u64,
    /// Every one of them that has lines is counted (binary files and
    /// submodules have none and are never waited for).
    pub counted: bool,
}

impl Totals {
    /// Over `files` (indices into the diff), as `viewport` has counted them.
    pub fn of(files: impl IntoIterator<Item = u32>, viewport: &DiffViewport) -> Totals {
        let changes = viewport.document().files();
        let mut t = Totals {
            counted: true,
            ..Totals::default()
        };
        for f in files {
            t.files += 1;
            match viewport.file_counts(f) {
                Some(c) => {
                    t.additions += u64::from(c.additions);
                    t.deletions += u64::from(c.deletions);
                }
                None => t.counted &= !changes.get(f as usize).is_some_and(has_lines),
            }
        }
        t
    }

    /// "+X −Y", grouped by thousands; "…" until counted.
    pub fn lines(&self) -> String {
        if !self.counted {
            return "…".into();
        }
        format!(
            "+{} −{}",
            group_digits(self.additions),
            group_digits(self.deletions)
        )
    }

    /// "12 files · +300 −20"; "1 file · …" until counted.
    pub fn summary(&self) -> String {
        let s = if self.files == 1 { "" } else { "s" };
        let files = group_digits(u64::from(self.files));
        format!("{files} file{s} · {}", self.lines())
    }
}

/// Whether the viewport counts `change`'s lines: text and symlinks, as it
/// now knows the kind (a file found binary when read has none).
fn has_lines(change: &FileChange) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink)
}

/// The totals' tooltip (design §11.15): without the categorized files (the
/// totals shown), with them, and them alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Breakdown {
    pub excluding: Totals,
    pub including: Totals,
    pub categorized: Totals,
}

/// `partition`'s breakdown, as `viewport` has counted it.
pub fn breakdown(partition: &Partition, viewport: &DiffViewport) -> Breakdown {
    let excluding = Totals::of(partition.main.iter().copied(), viewport);
    let categorized = Totals::of(
        partition
            .sections
            .iter()
            .flat_map(|s| s.files.iter().copied()),
        viewport,
    );
    let including = Totals {
        files: excluding.files + categorized.files,
        additions: excluding.additions + categorized.additions,
        deletions: excluding.deletions + categorized.deletions,
        counted: excluding.counted && categorized.counted,
    };
    Breakdown {
        excluding,
        including,
        categorized,
    }
}

/// The breakdown's three tooltip lines (design §11.15).
pub fn breakdown_lines(b: &Breakdown) -> [String; 3] {
    [
        format!("Without categorized files: {}", b.excluding.summary()),
        format!("With categorized files: {}", b.including.summary()),
        format!("Categorized only: {}", b.categorized.summary()),
    ]
}

/// The breakdown of `partition` as a tooltip, counted when it shows; `None`
/// while nothing is categorized (its lines would repeat the totals). Each
/// line is found as `tooltip: <line>`.
pub(crate) fn breakdown_tooltip(
    partition: &Arc<Partition>,
    viewport: &Entity<DiffViewport>,
) -> Option<impl Fn(&mut Window, &mut App) -> AnyView + use<>> {
    if partition.sections.is_empty() {
        return None;
    }
    let (partition, viewport) = (partition.clone(), viewport.clone());
    Some(move |window: &mut Window, cx: &mut App| {
        let lines = breakdown_lines(&breakdown(&partition, viewport.read(cx)));
        Tooltip::element(move |_, _| {
            v_flex().children(lines.clone().map(|line| {
                let selector = format!("tooltip: {line}");
                div().debug_selector(move || selector).child(line)
            }))
        })
        .build(window, cx)
    })
}

/// The chips as text: "6 tests · 1 generated".
pub(crate) fn chips_text(chips: &[Chip]) -> String {
    chips
        .iter()
        .map(|c| c.text.as_ref())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `chips` as small muted capsules (`pad::BADGE_X`, `text::SMALL`,
/// `gap::INLINE` apart), found as `"<name>: 6 tests · 1
/// generated"`; `None` without chips.
pub(crate) fn chips_element(name: &'static str, chips: &[Chip], cx: &App) -> Option<AnyElement> {
    if chips.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let joined = chips_text(chips);
    Some(
        h_flex()
            .debug_selector(move || format!("{name}: {joined}"))
            .flex_none()
            .gap(px(gap::INLINE))
            .children(chips.iter().map(|c| {
                div()
                    .px(px(pad::BADGE_X))
                    .rounded_full()
                    .bg(theme.muted)
                    .text_style(text::SMALL)
                    .text_color(theme.muted_foreground)
                    .child(c.text.clone())
            }))
            .into_any_element(),
    )
}
