//! File headers (design §11.6 "Sticky header", §6.4): the collapse chevron,
//! the path (`old → new` for renames), +/− counts, badges (similarity, mode,
//! binary, symlink, submodule, generated, LFS, plus the host's review flags),
//! the Viewed checkbox and the ⋯ menu.
//!
//! Headers are painted in their own layer after every row, so the header of
//! the first visible file can pin at the top while its body scrolls under it;
//! the next file's header pushes it up ([`crate::paint_rows::Painter`] picks
//! the position). The ⋯ menu is a gpui-kit `PopupMenu` (the host initializes
//! gpui-kit, as every Polygloss window does).

use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{
    Anchor, AnyElement, Bounds, ClipboardItem, Context, DismissEvent, Entity, Focusable as _,
    IntoElement as _, ParentElement as _, Pixels, Point, Subscription, Window, anchored, deferred,
    point, px,
};
use polygloss_diff::hunks::Block;
use polygloss_diff::{FileChange, FileKind, FileStatus, Side};

use crate::controls::{ControlAction, ControlLayer};
use crate::document::{BodyRow, FileState, RowKey, SizeHint};
use crate::paint_rows::{HEADERS, Painter};
use crate::text_cache::ShapedText;
use crate::view::{DiffViewport, ViewportEvent};

/// Color slots of cached labels (the same text in another color is another
/// cache entry).
pub(crate) const SLOT_HEADER: u8 = 0;
pub(crate) const SLOT_MUTED: u8 = 1;
pub(crate) const SLOT_REMOVED: u8 = 2;
pub(crate) const SLOT_ADDED: u8 = 3;
pub(crate) const SLOT_ACCENT: u8 = 4;
pub(crate) const SLOT_ON_ACCENT: u8 = 5;

/// Columns the title keeps before badges and counts give way.
const MIN_TITLE_COLUMNS: f32 = 12.0;

/// The header title: the path, or `old → new` for a rename.
pub(crate) fn header_title(change: &FileChange) -> String {
    match (&change.old_path, &change.new_path) {
        (Some(old), Some(new)) if old.text != new.text => format!("{} → {}", old.text, new.text),
        _ => change.display_path().to_owned(),
    }
}

/// Badges describing the change itself, left to right (design §6.4).
pub(crate) fn kind_badges(change: &FileChange, lfs: bool) -> Vec<String> {
    let mut badges = Vec::new();
    if change.status == FileStatus::Renamed
        && let Some(similarity) = change.similarity
    {
        badges.push(format!("{similarity}% similar"));
    }
    if let (Some(old), Some(new)) = (change.old_mode, change.new_mode)
        && old != new
    {
        badges.push(format!("{old} → {new}"));
    }
    match change.kind {
        FileKind::Binary => badges.push("binary".to_owned()),
        FileKind::Symlink => badges.push("symlink".to_owned()),
        FileKind::Submodule => badges.push("submodule".to_owned()),
        FileKind::Text => {}
    }
    if change.generated {
        badges.push("generated".to_owned());
    }
    if lfs {
        badges.push("LFS".to_owned());
    }
    badges
}

/// A badge to paint: its shaped text and its width with padding.
struct Badge {
    text: Rc<ShapedText>,
    width: f32,
}

impl Painter<'_> {
    /// Paints file `f`'s header with its top at `y` (`sticky` when pinned
    /// away from its place in the document) and records its controls.
    pub(crate) fn header(&mut self, f: u32, y: f32, h: f32, sticky: bool) {
        let files = self.files;
        let change = &files[f as usize];
        let theme = self.theme;
        let width = self.bounds.size.width.as_f32();
        let a = self.geometry.advance;
        let row_h = self.geometry.row_height;
        let ty = y + (h - row_h) / 2.0;
        self.frame.rows += 1;
        self.quad(HEADERS, 0.0, y, width, h, theme.header_background);
        self.quad(HEADERS, 0.0, y, width, 1.0, theme.border);
        self.quad(HEADERS, 0.0, y + h - 1.0, width, 1.0, theme.border);
        let area = self.bounds_at(0.0, y, width, h);
        self.frame.header_areas.push(area);

        // The chevron, left.
        let collapsed = self.doc.is_collapsed(f);
        let chevron = self.label(if collapsed { "▸" } else { "▾" }, SLOT_MUTED, theme.muted);
        self.text(HEADERS, a, ty, chevron);
        self.control(
            ControlAction::Collapse(f),
            ControlLayer::Header,
            0.0,
            y,
            3.0 * a,
            h,
        );

        // The ⋯ menu, right.
        let menu_x = width - 3.0 * a;
        let dots = self.label("⋯", SLOT_MUTED, theme.muted);
        let dots_x = menu_x + (3.0 * a - dots.shaped.width()) / 2.0;
        self.text(HEADERS, dots_x, ty, dots);
        self.control(
            ControlAction::Menu(f),
            ControlLayer::Header,
            menu_x,
            y,
            3.0 * a,
            h,
        );

        // The Viewed checkbox, left of the menu.
        let flags = self.flags.get(f as usize).copied().unwrap_or_default();
        let box_size = (row_h * 0.7).round();
        let viewed = self.label("Viewed", SLOT_HEADER, theme.header_foreground);
        let viewed_w = 0.5 * a + box_size + 0.5 * a + viewed.shaped.width() + 0.5 * a;
        let viewed_x = menu_x - viewed_w;
        let box_x = viewed_x + 0.5 * a;
        let box_rect = (box_x, y + (h - box_size) / 2.0, box_size, box_size);
        if flags.viewed {
            self.rounded(HEADERS, box_rect, theme.accent, Some(theme.accent), 3.0);
            let check = self.label("✓", SLOT_ON_ACCENT, theme.background);
            let check_x = box_x + (box_size - check.shaped.width()) / 2.0;
            self.text(HEADERS, check_x, ty, check);
        } else {
            self.rounded(
                HEADERS,
                box_rect,
                theme.background,
                Some(theme.line_number),
                3.0,
            );
        }
        self.text(HEADERS, box_x + box_size + 0.5 * a, ty, viewed);
        self.control(
            ControlAction::Viewed(f),
            ControlLayer::Header,
            viewed_x,
            y,
            viewed_w,
            h,
        );

        // Between them: the title, counts, change badges and review flags,
        // dropped in reverse priority until the title keeps some room.
        let left = 3.0 * a;
        let right = viewed_x - a;
        let title_text = header_title(change);
        let full_title = self.label(&title_text, SLOT_HEADER, theme.header_foreground);
        let counts = self.counts(f);
        let (added, removed) = match counts {
            Some((adds, dels)) => (
                (adds > 0).then(|| self.label(&format!("+{adds}"), SLOT_ADDED, theme.added_accent)),
                (dels > 0)
                    .then(|| self.label(&format!("−{dels}"), SLOT_REMOVED, theme.removed_accent)),
            ),
            None => (None, None),
        };
        let mut counts_w = 0.0;
        for t in added.iter().chain(removed.iter()) {
            counts_w += a + t.shaped.width();
        }
        if counts_w > 0.0 {
            counts_w += a;
        }
        let mut kinds: Vec<Badge> = kind_badges(change, self.special.is_lfs(f))
            .iter()
            .map(|b| self.badge(b, SLOT_MUTED, theme.muted))
            .collect();
        let mut flag_badges: Vec<Badge> = flags
            .badges()
            .iter()
            .map(|(b, accent)| {
                if *accent {
                    self.badge(b, SLOT_ACCENT, theme.accent)
                } else {
                    self.badge(b, SLOT_MUTED, theme.muted)
                }
            })
            .collect();
        let row_w = |badges: &[Badge]| {
            badges.iter().map(|b| b.width + 0.5 * a).sum::<f32>()
                + if badges.is_empty() { 0.0 } else { a }
        };
        let mut show_counts = true;
        let min_title = full_title.shaped.width().min(MIN_TITLE_COLUMNS * a);
        loop {
            let used =
                row_w(&kinds) + row_w(&flag_badges) + if show_counts { counts_w } else { 0.0 };
            if min_title + used <= right - left {
                break;
            }
            if kinds.pop().is_some() || flag_badges.pop().is_some() {
                continue;
            }
            if show_counts {
                show_counts = false;
                continue;
            }
            break;
        }
        let used = row_w(&kinds) + row_w(&flag_badges) + if show_counts { counts_w } else { 0.0 };
        let title = self.fit_title(&title_text, full_title, (right - left - used).max(a));
        self.text(HEADERS, left, ty, title.clone());
        let mut x = left + title.shaped.width();
        if show_counts {
            x += a;
            for t in added.iter().chain(removed.iter()) {
                x += a;
                self.text(HEADERS, x, ty, t.clone());
                x += t.shaped.width();
            }
        }
        if !kinds.is_empty() {
            x += a;
        }
        for b in &kinds {
            self.paint_badge(b, x, y, h);
            x += b.width + 0.5 * a;
        }
        // Review flags end where the Viewed checkbox starts.
        let mut fx = right - flag_badges.iter().map(|b| b.width + 0.5 * a).sum::<f32>() + 0.5 * a;
        for b in &flag_badges {
            self.paint_badge(b, fx, y, h);
            fx += b.width + 0.5 * a;
        }

        #[cfg(feature = "debug-inspect")]
        {
            use crate::paint_rows::{DebugContent, DebugRow};
            self.debug.push(DebugRow {
                y,
                height: h,
                styled: false,
                content: DebugContent::Header(title.clone()),
            });
            self.debug_headers.push(crate::debug::HeaderDebug {
                file_idx: f,
                y,
                sticky,
                title: title.text().to_owned(),
                counts: counts.filter(|_| show_counts),
                badges: kinds
                    .iter()
                    .chain(&flag_badges)
                    .map(|b| b.text.text().to_owned())
                    .collect(),
                viewed: flags.viewed,
                collapsed,
            });
        }
        #[cfg(not(feature = "debug-inspect"))]
        let _ = sticky;
    }

    /// Additions and deletions of file `f` once known (from its diff, or a
    /// size hint), unless both are zero.
    fn counts(&self, f: u32) -> Option<(u32, u32)> {
        let (adds, dels) = match self.doc.state(f) {
            FileState::Materialized(file) => (file.diff.additions, file.diff.deletions),
            _ => match self.doc.size_hint(f) {
                Some(SizeHint::Counts {
                    additions,
                    deletions,
                    ..
                }) => (additions, deletions),
                _ => return None,
            },
        };
        (adds + dels > 0).then_some((adds, dels))
    }

    fn badge(&mut self, text: &str, slot: u8, color: gpui_kit::Hsla) -> Badge {
        let text = self.label(text, slot, color);
        let width = text.shaped.width() + self.geometry.advance;
        Badge { text, width }
    }

    fn paint_badge(&mut self, badge: &Badge, x: f32, y: f32, h: f32) {
        let row_h = self.geometry.row_height;
        let badge_h = (row_h - 4.0).max(1.0);
        let rect = (x, y + (h - badge_h) / 2.0, badge.width, badge_h);
        self.rounded(HEADERS, rect, self.theme.badge_background, None, 4.0);
        let tx = x + 0.5 * self.geometry.advance;
        self.text(HEADERS, tx, y + (h - row_h) / 2.0, badge.text.clone());
    }

    /// The title, cut from the left with `…` to fit `avail` px (the end of a
    /// path is the part that tells files apart).
    fn fit_title(&mut self, title: &str, full: Rc<ShapedText>, avail: f32) -> Rc<ShapedText> {
        if full.shaped.width() <= avail {
            return full;
        }
        let a = self.geometry.advance;
        let chars: Vec<char> = title.chars().collect();
        let mut keep = ((avail / a).floor() as usize)
            .saturating_sub(1)
            .min(chars.len());
        loop {
            let cut: String = std::iter::once('…')
                .chain(chars[chars.len() - keep..].iter().copied())
                .collect();
            let shaped = self.label(&cut, SLOT_HEADER, self.theme.header_foreground);
            let over = shaped.shaped.width() - avail;
            if over <= 0.0 || keep == 0 {
                return shaped;
            }
            // Wide chars (CJK, emoji) take more than one column.
            keep = keep.saturating_sub(((over / a).ceil() as usize).max(1));
        }
    }
}

/// The ⋯ menu of one file, while open.
pub(crate) struct HeaderMenu {
    pub file_idx: u32,
    view: Entity<PopupMenu>,
    /// Where the menu's top-right corner goes: the button's bottom-right.
    position: Point<Pixels>,
    /// Labels and whether each is enabled, for `ViewportDebug`.
    pub items: Vec<(&'static str, bool)>,
    _dismiss: Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    OpenInEditor,
    CommentOnFile,
    CopyPath,
    ExpandAll,
    LoadDiff,
}

impl DiffViewport {
    /// Collapses file `file_idx` to its header, or expands it. Nothing on
    /// screen moves: collapsing the file at the top leaves its header there.
    pub fn set_collapsed(&mut self, file_idx: u32, collapsed: bool, cx: &mut Context<Self>) {
        if file_idx >= self.doc.len() || self.doc.is_collapsed(file_idx) == collapsed {
            return;
        }
        self.doc.set_collapsed(file_idx, collapsed);
        self.after_scroll(cx);
    }

    /// Collapsed files, in order.
    pub fn collapsed(&self) -> Vec<u32> {
        (0..self.doc.len())
            .filter(|&f| self.doc.is_collapsed(f))
            .collect()
    }

    /// Opens file `f`'s ⋯ menu under the button at `button` (window
    /// coordinates).
    pub(crate) fn open_menu(
        &mut self,
        f: u32,
        button: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = [
            (MenuItem::OpenInEditor, "Open in editor", true),
            (MenuItem::CommentOnFile, "Comment on file", true),
            (MenuItem::CopyPath, "Copy path", true),
            (MenuItem::ExpandAll, "Expand all", self.can_expand(f)),
            (MenuItem::LoadDiff, "Load diff", self.can_load_diff(f)),
        ];
        let this = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            for (item, label, enabled) in items {
                if item == MenuItem::ExpandAll {
                    menu = menu.separator();
                }
                let this = this.clone();
                menu = menu.item(PopupMenuItem::new(label).disabled(!enabled).on_click(
                    move |_, _, cx| {
                        this.update(cx, |v, cx| v.menu_action(f, item, cx)).ok();
                    },
                ));
            }
            menu
        });
        let dismiss = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, _, cx| {
            this.menu = None;
            cx.notify();
        });
        window.focus(&menu.focus_handle(cx), cx);
        self.menu = Some(HeaderMenu {
            file_idx: f,
            view: menu,
            position: point(button.right(), button.bottom()),
            items: items.iter().map(|(_, l, e)| (*l, *e)).collect(),
            _dismiss: dismiss,
        });
        cx.notify();
    }

    fn menu_action(&mut self, f: u32, item: MenuItem, cx: &mut Context<Self>) {
        match item {
            MenuItem::OpenInEditor => {
                let (side, line) = self.editor_target(f);
                cx.emit(ViewportEvent::OpenInEditor {
                    file_idx: f,
                    side,
                    line,
                });
            }
            MenuItem::CommentOnFile => cx.emit(ViewportEvent::FileCommentRequested(f)),
            MenuItem::CopyPath => {
                let path = self.files[f as usize].display_path().to_owned();
                cx.write_to_clipboard(ClipboardItem::new_string(path));
            }
            MenuItem::ExpandAll => self.expand_file(f, cx),
            MenuItem::LoadDiff => {
                self.load_diff(f, cx);
                cx.emit(ViewportEvent::LoadDiffRequested(f));
            }
        }
    }

    /// The open menu, drawn above everything else.
    pub(crate) fn menu_element(&self) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu.view.clone()),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// Whether file `f` has hidden context to reveal.
    fn can_expand(&self, f: u32) -> bool {
        self.doc.state(f).is_materialized()
            && self.doc.file_layout(f).is_some_and(|layout| {
                layout
                    .rows()
                    .iter()
                    .any(|r| matches!(r, BodyRow::Gap { .. }))
            })
    }

    /// Where "Open in editor" points: the new side (the old one for a deleted
    /// file), at the line at the top of the viewport when it is in this file,
    /// else at the file's first change.
    fn editor_target(&self, f: u32) -> (Side, u32) {
        let change = &self.files[f as usize];
        let side = if change.new_path.is_none() {
            Side::Old
        } else {
            Side::New
        };
        let anchor = self.doc.anchor();
        if anchor.file_idx == f
            && let RowKey::Line { side: s, line } = anchor.row
            && s == side
        {
            return (side, line);
        }
        if let FileState::Materialized(file) = self.doc.state(f) {
            let first = file
                .diff
                .hunks
                .iter()
                .flat_map(|h| &h.blocks)
                .find_map(|b| match b {
                    Block::Change { old, new } => Some(match side {
                        Side::Old => old.start,
                        Side::New => new.start,
                    }),
                    Block::Equal { .. } => None,
                });
            if let Some(line) = first {
                return (side, line);
            }
        }
        (side, 0)
    }
}
