//! File headers (design §11.6 "File header", §6.4): the collapse chevron,
//! the path in the code font with the directory dim and the name bold
//! (`old → new` for renames), muted kind pills (similarity, mode, binary,
//! symlink, submodule, generated, LFS), then right-aligned the host's
//! review-state pills, open in editor, the `+a −d` pill, the Viewed pill and
//! the ⋯ menu. Icons are Lucide SVGs ([`Painter::icon`]); pills and "Viewed"
//! use the UI font.
//!
//! Geometry (ADR-0031 C2): the interior is a code row plus 24 pt
//! (`card::header`) inside the strip's top border and the separator; the
//! rest is fixed points from the card's inner edges. The chevron's 16 pt box
//! at `card::CHEVRON_X`, the path at `card::PATH_X`, the ⋯ button ending
//! `edge::CARD_TRAILING` from the inner right edge; the chevron, open in
//! editor and ⋯ are `height::SM` buttons around 16 pt icons, pills
//! `height::SM` capsules, Viewed a bordered `height::MD` capsule; sibling
//! controls `gap::CONTROLS` apart.
//!
//! Headers are painted in their own layer after every row, so the header of
//! the first visible file can pin at the top while its body scrolls under it;
//! the end of its body pushes it up ([`crate::paint_rows::Painter`] picks the
//! position). On a card ([`crate::card`]) a header in place takes the card's
//! top corners; pinned it is square and flush at the top edge, spans the
//! card and has a bottom border. The ⋯ menu is a gpui-kit `PopupMenu` (the
//! host initializes gpui-kit, as every Polygloss window does).

use std::borrow::Cow;
use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{
    Anchor, AnyElement, AnyWindowHandle, App, Bounds, ClipboardItem, Context, DismissEvent, Entity,
    FocusHandle, Focusable as _, Font, FontWeight, Hsla, IntoElement as _, ParentElement as _,
    Pixels, Point, Subscription, Window, anchored, deferred, point, px,
};
use polygloss_diff::hunks::Block;
use polygloss_diff::{FileChange, FileKind, FileStatus, Side};

use crate::controls::{ControlAction, ControlLayer};
use crate::debug::TitleStyle;
use crate::document::{BodyRow, FileState, SizeHint};
use crate::numbers::group_digits;
use crate::paint_rows::{Frame, HEADERS, Painter};
use crate::space::{card, edge, gap, height, pad, radius, size};
use crate::text_cache::{ShapedText, Shaper, TextKey};
use crate::title::Title;
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

/// Badges describing the change itself, left to right (design §6.4).
pub(crate) fn kind_badges(change: &FileChange, lfs: bool) -> Vec<Cow<'static, str>> {
    let mut badges = Vec::new();
    if change.status == FileStatus::Renamed
        && let Some(similarity) = change.similarity
    {
        badges.push(Cow::Owned(format!("{similarity}% similar")));
    }
    if let (Some(old), Some(new)) = (change.old_mode, change.new_mode)
        && old != new
    {
        badges.push(Cow::Owned(format!("{old} → {new}")));
    }
    match change.kind {
        FileKind::Binary => badges.push(Cow::Borrowed("binary")),
        FileKind::Symlink => badges.push(Cow::Borrowed("symlink")),
        FileKind::Submodule => badges.push(Cow::Borrowed("submodule")),
        FileKind::Text => {}
    }
    if change.generated {
        badges.push(Cow::Borrowed("generated"));
    }
    if lfs {
        badges.push(Cow::Borrowed("LFS"));
    }
    badges
}

pub(crate) const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub(crate) const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
const ELLIPSIS: &str = "icons/ellipsis.svg";
const OPEN_IN_EDITOR: &str = "icons/square-arrow-out-up-right.svg";
const VIEWED_BOX: &str = "icons/square.svg";
const VIEWED_CHECKED: &str = "icons/square-check.svg";

/// A pill to paint: its label, its icon and its width with padding.
pub(crate) struct Pill {
    pub text: Rc<ShapedText>,
    icon: Option<(&'static str, Hsla)>,
    pub width: f32,
}

impl Painter<'_> {
    /// Paints file `f`'s header with its top at `y` (`sticky` when pinned
    /// away from its place in the document) and records its controls.
    ///
    /// Left to right: the chevron, the title, the kind pills; right-aligned
    /// the review-state pills, open in editor, the `+a −d` pill, Viewed and
    /// ⋯. As it narrows the kind pills go first (right to left), then the
    /// review-state pills, then the `+a −d` pill; the title keeps 12
    /// columns. At very narrow widths the "Viewed" label gives way, then
    /// open in editor and the Viewed box (when they would not fit right of
    /// the chevron).
    pub(crate) fn header(&mut self, f: u32, y: f32, h: f32, sticky: bool) {
        let files = self.files;
        let change = &files[f as usize];
        let theme = self.theme;
        // Content goes across the card's inner width (the whole width in the
        // flat layout).
        let (x0, width) = self.inner_x_w();
        let a = self.geometry.advance;
        let row_h = self.geometry.row_height;
        let ty = y + (h - row_h) / 2.0;
        let icon_y = y + (h - size::ICON).max(0.0) / 2.0;
        // The chevron, open in editor and ⋯: SM buttons centered on their
        // 16 pt icons.
        let button = height::SM;
        let button_y = y + (h - button) / 2.0;
        let icon_inset = (button - size::ICON) / 2.0;
        self.frame.rows += 1;
        self.header_strip(f, y, h, sticky);

        // The chevron, left; ⋯, right.
        let collapsed = self.doc.is_collapsed(f);
        let chevron = if collapsed {
            CHEVRON_RIGHT
        } else {
            CHEVRON_DOWN
        };
        let chevron_x = x0 + card::CHEVRON_X;
        self.icon(chevron, chevron_x, icon_y, size::ICON, theme.muted);
        let action = ControlAction::Collapse(f);
        let chevron_button = chevron_x - icon_inset;
        self.control(
            action,
            ControlLayer::Header,
            chevron_button,
            button_y,
            button,
            button,
        );
        let menu_x = x0 + width - edge::CARD_TRAILING - button;
        let ellipsis_x = menu_x + icon_inset;
        self.icon(ELLIPSIS, ellipsis_x, icon_y, size::ICON, theme.muted);
        let action = ControlAction::Menu(f);
        self.control(
            action,
            ControlLayer::Header,
            menu_x,
            button_y,
            button,
            button,
        );

        // Viewed (labeled while the title keeps its room, else its box
        // alone) and open in editor, each while it fits right of the
        // chevron.
        let left = x0 + card::PATH_X;
        let title = Title::of(change);
        let full_title = self.title(f, &title, f32::INFINITY);
        let min_title = full_title.shaped.width().min(MIN_TITLE_COLUMNS * a);
        let flags = self.flags.get(f as usize).copied().unwrap_or_default();
        let viewed_label = self.ui_label("Viewed", SLOT_HEADER, theme.header_foreground);
        let box_w = pad::PILL_X + size::ICON + pad::PILL_X;
        let labeled_w = box_w + gap::ICON_LABEL + viewed_label.shaped.width();
        let between = gap::CONTROLS;
        let fixed = |viewed_w: f32| between + viewed_w + between + button + between;
        let (viewed_w, editor) = if menu_x - fixed(labeled_w) - left >= min_title {
            (Some(labeled_w), true)
        } else if menu_x - between - box_w >= left {
            (Some(box_w), menu_x - fixed(box_w) + between >= left)
        } else {
            (None, false)
        };

        let counts = self.counts(f);
        let counts_pill = counts.map(|(adds, dels)| {
            let added = self.count(adds, true);
            let removed = self.count(dels, false);
            let cluster = added.shaped.width() + gap::INLINE + removed.shaped.width();
            let w = pad::PILL_X + cluster + pad::PILL_X;
            (added, removed, w)
        });
        let mut kinds: Vec<Pill> = kind_badges(change, self.special.is_lfs(f))
            .iter()
            .map(|b| self.pill(b, None, SLOT_MUTED, theme.muted))
            .collect();
        let mut reviews: Vec<Pill> = flags
            .badges()
            .iter()
            .map(|b| {
                let (slot, color) = if b.accent {
                    (SLOT_ACCENT, theme.accent)
                } else {
                    (SLOT_MUTED, theme.muted)
                };
                self.pill(&b.text, b.icon, slot, color)
            })
            .collect();
        // Where the title and kind pills must end: left of the right
        // cluster, which the review pills and the counts widen.
        let limit = |reviews: &[Pill], counts: bool| {
            let mut rx = menu_x;
            if let Some(w) = viewed_w {
                rx -= between + w;
            }
            if let Some((_, _, w)) = counts_pill.as_ref().filter(|_| counts) {
                rx -= between + w;
            }
            if editor {
                rx -= between + button;
            }
            rx - reviews.iter().map(|p| between + p.width).sum::<f32>() - between
        };
        let kinds_w = |kinds: &[Pill]| kinds.iter().map(|p| between + p.width).sum::<f32>();
        let mut show_counts = counts_pill.is_some();
        while left + min_title + kinds_w(&kinds) > limit(&reviews, show_counts) {
            if kinds.pop().is_some() || reviews.pop().is_some() {
                continue;
            }
            if !show_counts {
                break;
            }
            show_counts = false;
        }

        // Right to left: Viewed, the counts, open in editor, review pills.
        let pill_h = height::SM;
        let pill_y = y + (h - pill_h) / 2.0;
        let mut rx = menu_x;
        if let Some(w) = viewed_w {
            rx -= between + w;
            self.viewed_pill(f, flags.viewed, rx, y, h, w, viewed_label);
        }
        if let Some((added, removed, w)) = counts_pill.filter(|_| show_counts) {
            rx -= between + w;
            let capsule = radius::capsule(pill_h);
            let background = theme.pill_background;
            self.rounded(HEADERS, (rx, pill_y, w, pill_h), background, None, capsule);
            let tx = rx + pad::PILL_X;
            let removed_x = tx + added.shaped.width() + gap::INLINE;
            self.text(HEADERS, tx, ty, added);
            self.text(HEADERS, removed_x, ty, removed);
        }
        if editor {
            rx -= between + button;
            self.icon(
                OPEN_IN_EDITOR,
                rx + icon_inset,
                icon_y,
                size::ICON,
                theme.muted,
            );
            let action = ControlAction::OpenInEditor(f);
            self.control(action, ControlLayer::Header, rx, button_y, button, button);
        }
        rx -= between;
        for p in reviews.iter().rev() {
            rx -= p.width;
            self.paint_pill(p, rx, pill_y, pill_h, ty);
            rx -= between;
        }

        // The title, cut to what is left, and the kind pills after it.
        let avail = (rx - left - kinds_w(&kinds)).max(a);
        let fitted = self.title(f, &title, avail);
        self.text(HEADERS, left, ty, fitted.clone());
        let mut x = left + fitted.shaped.width();
        for p in &kinds {
            x += between;
            self.paint_pill(p, x, pill_y, pill_h, ty);
            x += p.width;
        }

        #[cfg(feature = "debug-inspect")]
        {
            use crate::paint_rows::{DebugContent, DebugRow};
            let painted = fitted.text();
            let runs = if painted == title.text {
                title.styled()
            } else {
                let keep = painted.chars().count().saturating_sub(1);
                title.cut(keep).styled()
            };
            self.debug.push(DebugRow {
                y,
                height: h,
                styled: false,
                content: DebugContent::Header(fitted.clone()),
            });
            self.debug_headers.push(crate::debug::HeaderDebug {
                file_idx: f,
                y,
                sticky,
                title: painted.to_owned(),
                title_runs: runs,
                counts: counts.filter(|_| show_counts),
                badges: kinds
                    .iter()
                    .chain(&reviews)
                    .map(|p| p.text.text().to_owned())
                    .collect(),
                viewed: flags.viewed,
                collapsed,
            });
        }
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

    /// The Viewed pill of file `f` at `x`, `w` wide: a bordered
    /// `height::MD` capsule holding a box (checked: `square-check` in the
    /// accent color) and, when it is wide enough, "Viewed" in the UI font.
    #[allow(clippy::too_many_arguments)]
    fn viewed_pill(
        &mut self,
        f: u32,
        checked: bool,
        x: f32,
        y: f32,
        h: f32,
        w: f32,
        label: Rc<ShapedText>,
    ) {
        let theme = self.theme;
        let pill_h = height::MD.min(h);
        let pill_y = y + (h - pill_h) / 2.0;
        let rect = (x, pill_y, w, pill_h);
        let capsule = radius::capsule(pill_h);
        let (background, border) = (theme.card_background, Some(theme.card_border));
        self.rounded(HEADERS, rect, background, border, capsule);
        let (icon, color) = if checked {
            (VIEWED_CHECKED, theme.accent)
        } else {
            (VIEWED_BOX, theme.line_number)
        };
        let box_x = x + pad::PILL_X;
        self.icon(icon, box_x, y + (h - size::ICON) / 2.0, size::ICON, color);
        if w > pad::PILL_X + size::ICON + pad::PILL_X {
            let ty = y + (h - self.geometry.row_height) / 2.0;
            let label_x = box_x + size::ICON + gap::ICON_LABEL;
            self.text(HEADERS, label_x, ty, label);
        }
        let action = ControlAction::Viewed(f);
        self.rounded_control(action, ControlLayer::Header, rect, capsule);
    }

    /// `+n` (added) or `−n` (removed) with its thousands grouped, in its
    /// stat color, cached by value (no string is built unless it has to be
    /// shaped).
    fn count(&mut self, n: u32, added: bool) -> Rc<ShapedText> {
        let (slot, color, sign) = if added {
            (SLOT_ADDED, self.theme.stat_added, '+')
        } else {
            (SLOT_REMOVED, self.theme.stat_removed, '−')
        };
        let shaper = Shaper {
            theme: self.theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        self.cache
            .get_or_shape(TextKey::Count { n, color: slot }, || {
                shaper.label(&format!("{sign}{}", group_digits(u64::from(n))), color)
            })
    }

    /// A pill with `text` in the UI font and `color`, and `icon` before it,
    /// `pad::PILL_X` inside each end.
    pub(crate) fn pill(
        &mut self,
        text: &str,
        icon: Option<&'static str>,
        slot: u8,
        color: Hsla,
    ) -> Pill {
        let text = self.ui_label(text, slot, color);
        let icon_w = if icon.is_some() {
            size::ICON_SM + gap::ICON_LABEL
        } else {
            0.0
        };
        let width = pad::PILL_X + icon_w + text.shaped.width() + pad::PILL_X;
        Pill {
            text,
            icon: icon.map(|i| (i, color)),
            width,
        }
    }

    /// Paints `pill` as an `h`-tall capsule at `(x, y)`, its text's top at
    /// `ty`: `pad::PILL_X` to its first box, a 14 pt icon `gap::ICON_LABEL`
    /// before the text.
    pub(crate) fn paint_pill(&mut self, pill: &Pill, x: f32, y: f32, h: f32, ty: f32) {
        let rect = (x, y, pill.width, h);
        let background = self.theme.pill_background;
        self.rounded(HEADERS, rect, background, None, radius::capsule(h));
        let mut tx = x + pad::PILL_X;
        if let Some((icon, color)) = pill.icon {
            let icon_y = y + (h - size::ICON_SM) / 2.0;
            self.icon(icon, tx, icon_y, size::ICON_SM, color);
            tx += size::ICON_SM + gap::ICON_LABEL;
        }
        self.text(HEADERS, tx, ty, pill.text.clone());
    }

    /// File `f`'s title fitted to `avail` px, cached by file and width: the
    /// whole title when it fits, else cut from the left with `…`.
    fn title(&mut self, f: u32, title: &Title, avail: f32) -> Rc<ShapedText> {
        let theme = self.theme;
        let mut bold = self.font.clone();
        bold.weight = FontWeight::BOLD;
        let runs = |t: &Title| -> Vec<(usize, Hsla, Font)> {
            t.runs
                .iter()
                .map(|&(len, style)| match style {
                    TitleStyle::Dim => (len, theme.muted, self.font.clone()),
                    TitleStyle::Bold => (len, theme.header_foreground, bold.clone()),
                })
                .collect()
        };
        let shaper = Shaper {
            theme,
            font: self.font,
            geometry: self.geometry,
            text_system: &self.text_system,
        };
        let full = TextKey::Title {
            file_idx: f,
            width: u32::MAX,
        };
        let whole = self
            .cache
            .get_or_shape(full, || shaper.runs(&title.text, &runs(title)));
        if whole.shaped.width() <= avail {
            return whole;
        }
        let avail = avail.floor();
        let key = TextKey::Title {
            file_idx: f,
            width: avail as u32,
        };
        let a = self.geometry.advance;
        self.cache.get_or_shape(key, || {
            let chars = title.text.chars().count();
            let mut keep = ((avail / a).floor() as usize).saturating_sub(1).min(chars);
            loop {
                let cut = title.cut(keep);
                let shaped = shaper.runs(&cut.text, &runs(&cut));
                let over = shaped.shaped.width() - avail;
                if over <= 0.0 || keep == 0 {
                    return shaped;
                }
                // Wide chars (CJK, emoji) take more than one column.
                keep = keep.saturating_sub(((over / a).ceil() as usize).max(1));
            }
        })
    }
}

/// The ⋯ menu of one file, while open.
pub(crate) struct HeaderMenu {
    pub file_idx: u32,
    view: Entity<PopupMenu>,
    /// Where the menu's top-right corner goes: the button's bottom-right
    /// (window coordinates, as of the last frame).
    position: Point<Pixels>,
    /// Labels and whether each is enabled, for `ViewportDebug`.
    #[cfg(feature = "debug-inspect")]
    pub items: Vec<(&'static str, bool)>,
    /// What had focus before the menu took it (and its window): it gets it
    /// back when the menu closes.
    previous_focus: Option<(AnyWindowHandle, FocusHandle)>,
    _dismiss: Subscription,
}

impl HeaderMenu {
    /// Hands focus back to what had it before the menu opened, unless
    /// something else took it meanwhile (an item's handler focusing a
    /// composer).
    fn restore_focus(&self, window: &mut Window, cx: &mut App) {
        let Some((_, previous)) = &self.previous_focus else {
            return;
        };
        let menu_focus = self.view.focus_handle(cx);
        if window.focused(cx).is_none() || menu_focus.contains_focused(window, cx) {
            window.focus(previous, cx);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    OpenInEditor,
    CommentOnFile,
    CopyPath,
    ExpandAll,
    LoadDiff,
    HighlightAnyway,
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
    /// coordinates). The menu takes focus and hands it back when it closes.
    pub(crate) fn open_menu(
        &mut self,
        f: u32,
        button: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Replacing an open menu keeps what had focus before it.
        let previous_focus = match self.menu.take() {
            Some(open) => open.previous_focus,
            None => window.focused(cx).map(|f| (window.window_handle(), f)),
        };
        let mut items = vec![
            (MenuItem::OpenInEditor, "Open in editor", true),
            (MenuItem::CommentOnFile, "Comment on file", true),
            (MenuItem::CopyPath, "Copy path", true),
            (MenuItem::ExpandAll, "Expand all", self.can_expand(f)),
            (MenuItem::LoadDiff, "Load diff", self.can_load_diff(f)),
        ];
        // Only while a side renders plain for its size (design §11.11).
        if self.syntax_skipped(f) {
            items.push((MenuItem::HighlightAnyway, "Highlight anyway", true));
        }
        #[cfg(feature = "debug-inspect")]
        let debug_items = items.iter().map(|(_, l, e)| (*l, *e)).collect();
        let this = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            for &(item, label, enabled) in &items {
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
        // Escape, a press outside the menu, a chosen item.
        let dismiss = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
            if let Some(menu) = this.menu.take() {
                menu.restore_focus(window, cx);
            }
            cx.notify();
        });
        window.focus(&menu.focus_handle(cx), cx);
        self.menu = Some(HeaderMenu {
            file_idx: f,
            view: menu,
            position: point(button.right(), button.bottom()),
            #[cfg(feature = "debug-inspect")]
            items: debug_items,
            previous_focus,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    /// The file whose ⋯ menu is open.
    pub fn menu_file(&self) -> Option<u32> {
        self.menu.as_ref().map(|m| m.file_idx)
    }

    /// The file the keyboard's file actions act on (`m`, `z`): the
    /// cursor's, else the one at the top.
    pub fn current_file(&self) -> u32 {
        self.cursor
            .pos
            .map_or(self.doc.anchor().file_idx, |p| p.file_idx)
    }

    /// Opens file `f`'s ⋯ menu from the keyboard (`m`), under its button as
    /// last painted. When the file's header is off screen it is scrolled
    /// in first and the menu opens with the frame that paints it.
    pub fn open_file_menu(&mut self, f: u32, window: &mut Window, cx: &mut Context<Self>) {
        if f >= self.doc.len() {
            return;
        }
        match self.menu_button(f) {
            Some(button) => self.open_menu(f, button, window, cx),
            None => {
                self.pending_menu = Some(f);
                self.scroll_to(crate::ScrollTarget::File(f), cx);
            }
        }
    }

    /// Collapses file `f` to its header, or expands it (`z`).
    pub fn toggle_collapsed(&mut self, f: u32, cx: &mut Context<Self>) {
        if f < self.doc.len() {
            let collapsed = self.doc.is_collapsed(f);
            self.set_collapsed(f, !collapsed, cx);
        }
    }

    /// File `f`'s ⋯ button as the last frame painted it.
    fn menu_button(&self, f: u32) -> Option<Bounds<Pixels>> {
        self.frame_pool
            .as_ref()?
            .controls
            .iter()
            .find(|c| c.action == ControlAction::Menu(f))
            .map(|c| c.bounds)
    }

    /// Closes the ⋯ menu, if open, and hands focus back once the current
    /// update is done (this has no window to focus with).
    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        if let Some((window, _)) = &menu.previous_focus {
            let window = *window;
            cx.defer(move |cx| {
                window
                    .update(cx, |_, window, cx| menu.restore_focus(window, cx))
                    .ok();
            });
        }
        cx.notify();
    }

    /// Keeps the open menu under its ⋯ button after the frame moved the
    /// button (a resize, a relayout by the host), or closes the menu when
    /// the button is no longer painted. Called in prepaint, when the menu
    /// for this frame is already rendered, so the change shows next frame.
    pub(crate) fn follow_menu_button(
        &mut self,
        frame: &Frame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(f) = self.pending_menu.take() {
            let button = frame
                .controls
                .iter()
                .find(|c| c.action == ControlAction::Menu(f))
                .map(|c| c.bounds);
            if let Some(button) = button {
                self.open_menu(f, button, window, cx);
            }
        }
        let Some(menu) = &mut self.menu else {
            return;
        };
        let button = frame
            .controls
            .iter()
            .find(|c| c.action == ControlAction::Menu(menu.file_idx))
            .map(|c| point(c.bounds.right(), c.bounds.bottom()));
        match button {
            Some(position) if position == menu.position => return,
            Some(position) => menu.position = position,
            None => self.close_menu(cx),
        }
        let view = cx.entity_id();
        window.on_next_frame(move |_, cx| cx.notify(view));
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
            MenuItem::HighlightAnyway => self.highlight_anyway(f, cx),
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
                    .snap_to_window_with_margin(px(edge::OVERLAY))
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

    /// The first line of `side` shown below file `f`'s pinned header (a gap
    /// row counts as its first hidden line), or `None` unless `f`'s body is
    /// scrolled under its header.
    fn first_line_below_header(&self, f: u32, side: Side) -> Option<u32> {
        // The header is pinned at the viewport's top edge, so the body pixel
        // at its bottom edge is the viewport top's offset from its own top.
        let body_y = self.doc.scroll_top() - self.doc.header_top(f);
        let layout = self.doc.file_layout(f)?;
        if self.doc.is_collapsed(f) || body_y <= 0.0 || body_y >= layout.height() {
            return None;
        }
        let (first, _) = layout.row_at(body_y);
        layout.rows()[first..]
            .iter()
            .find_map(|row| match (*row, side) {
                (BodyRow::Line { old, .. }, Side::Old) => old,
                (BodyRow::Line { new, .. }, Side::New) => new,
                (BodyRow::Gap { old_start, .. }, Side::Old) => Some(old_start),
                (BodyRow::Gap { new_start, .. }, Side::New) => Some(new_start),
                _ => None,
            })
    }

    /// Where "Open in editor" points: the new side (the old one for a deleted
    /// file), at the first line shown below the pinned header while this
    /// file's body scrolls under it, else at the file's first change.
    pub(crate) fn editor_target(&self, f: u32) -> (Side, u32) {
        let change = &self.files[f as usize];
        let side = if change.new_path.is_none() {
            Side::Old
        } else {
            Side::New
        };
        if let Some(line) = self.first_line_below_header(f, side) {
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
