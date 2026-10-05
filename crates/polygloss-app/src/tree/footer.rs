//! The Files segment's footer (design §11.5): "Total: +X −Y" over the files
//! it is given, grouped by thousands, and "Total: …" until each of them that
//! has lines is counted. Binary files and submodules have none and add
//! nothing. The tree renders it again when counts land
//! (`ViewportEvent::CountsUpdated`), never per frame.
//!
//! Debug selector: `tree-footer: <its text>`.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex};
use gpui_kit::{
    AnyElement, App, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, div, px,
};
use polygloss_diff::{FileChange, FileKind};
use polygloss_viewport::{DiffViewport, group_digits};

/// The footer's height (pt).
const HEIGHT: f32 = 38.0;

/// "Total: +X −Y" over `files` (indices into the diff); "Total: …" until
/// every one of them is counted.
pub fn footer(files: &[u32], viewport: &Entity<DiffViewport>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row = h_flex()
        .flex_none()
        .h(px(HEIGHT))
        .px_3()
        .gap_1()
        .border_t_1()
        .border_color(theme.border)
        .text_sm()
        .child(div().text_color(theme.muted_foreground).child("Total:"));
    match totals(files, viewport.read(cx)) {
        Some((added, removed)) => {
            let colors = crate::theme::viewport_theme(cx);
            let added = format!("+{}", group_digits(added));
            let removed = format!("−{}", group_digits(removed));
            let text = format!("Total: {added} {removed}");
            // The counts in the code font, as in the rows.
            let count = |text: String, color: Hsla| {
                div()
                    .font_family(theme.mono_font_family.clone())
                    .font_semibold()
                    .text_color(color)
                    .child(text)
            };
            row.debug_selector(move || format!("tree-footer: {text}"))
                .child(count(added, colors.stat_added))
                .child(count(removed, colors.stat_removed))
                .into_any_element()
        }
        None => row
            .debug_selector(|| "tree-footer: Total: …".into())
            .child(div().text_color(theme.muted_foreground).child("…"))
            .into_any_element(),
    }
}

/// Lines added and removed over `files`; `None` while one of them that has
/// lines is not counted yet.
fn totals(files: &[u32], viewport: &DiffViewport) -> Option<(u64, u64)> {
    let changes = viewport.document().files();
    files.iter().try_fold((0, 0), |(added, removed), &f| {
        match viewport.file_counts(f) {
            Some(c) => Some((
                added + u64::from(c.additions),
                removed + u64::from(c.deletions),
            )),
            None if changes.get(f as usize).is_some_and(has_lines) => None,
            None => Some((added, removed)),
        }
    })
}

/// Whether the viewport counts `change`'s lines: text and symlinks, as it
/// now knows the kind (a file found binary when read has none).
fn has_lines(change: &FileChange) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink)
}
