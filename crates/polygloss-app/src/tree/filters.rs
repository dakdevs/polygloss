//! The file tree's filter (design §11.5, OQ-44): unviewed, has comments,
//! status (A/M/D/R), extension, and a fuzzy filter box ranked by
//! `nucleo-matcher` (the same matcher the ⌘P finder uses, [`Fuzzy`]); and the
//! filter menu that toggles them. The Files segment has one [`TreeFilter`];
//! every panel applies it to its own files.

use std::collections::BTreeSet;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{Context, WeakEntity, px};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use polygloss_diff::{FileChange, FileStatus};
use polygloss_viewport::FileFlags;

use super::FileTree;
use crate::space::layout;

/// The status filter's choices (design §11.5: A/M/D/R). A type change
/// counts as modified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatusFilter {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 4] = [
        StatusFilter::Added,
        StatusFilter::Modified,
        StatusFilter::Deleted,
        StatusFilter::Renamed,
    ];

    /// The filter a file of `status` falls under.
    pub fn of(status: FileStatus) -> StatusFilter {
        match status {
            FileStatus::Added => StatusFilter::Added,
            FileStatus::Deleted => StatusFilter::Deleted,
            FileStatus::Renamed => StatusFilter::Renamed,
            FileStatus::Modified | FileStatus::TypeChanged => StatusFilter::Modified,
        }
    }

    /// The menu's label.
    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::Added => "Added",
            StatusFilter::Modified => "Modified",
            StatusFilter::Deleted => "Deleted",
            StatusFilter::Renamed => "Renamed",
        }
    }
}

/// Which files the tree's panels show. Empty sets mean "any".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeFilter {
    /// Only files not marked Viewed.
    pub unviewed: bool,
    /// Only files with open threads.
    pub has_comments: bool,
    pub statuses: BTreeSet<StatusFilter>,
    /// Lowercase extensions without the dot; `""` = files without one.
    pub extensions: BTreeSet<String>,
    /// The fuzzy filter box's text.
    pub query: String,
}

impl TreeFilter {
    /// Whether any part of it hides files.
    pub fn is_active(&self) -> bool {
        self.unviewed
            || self.has_comments
            || !self.statuses.is_empty()
            || !self.extensions.is_empty()
            || !self.query.trim().is_empty()
    }

    /// Whether any filter of the ⋯ menu (not the text box) is on.
    pub fn menu_active(&self) -> bool {
        self.unviewed
            || self.has_comments
            || !self.statuses.is_empty()
            || !self.extensions.is_empty()
    }

    /// The files of `among` (indices into `files`, kept in their order)
    /// this filter keeps. `flags` has one entry per file (missing entries
    /// count as all-false).
    pub fn apply(&self, files: &[FileChange], flags: &[FileFlags], among: &[u32]) -> Vec<u32> {
        let mut fuzzy = Fuzzy::new(&self.query);
        among
            .iter()
            .copied()
            .filter(|&i| {
                let Some(f) = files.get(i as usize) else {
                    return false;
                };
                let flags = flags.get(i as usize).copied().unwrap_or_default();
                self.keeps(f, &flags)
                    && fuzzy
                        .as_mut()
                        .is_none_or(|z| z.score(f.display_path()).is_some())
            })
            .collect()
    }

    /// Whether `file` passes every filter but the text box.
    fn keeps(&self, file: &FileChange, flags: &FileFlags) -> bool {
        (!self.unviewed || !flags.viewed)
            && (!self.has_comments || flags.open_threads > 0)
            && (self.statuses.is_empty() || self.statuses.contains(&StatusFilter::of(file.status)))
            && (self.extensions.is_empty()
                || self.extensions.contains(&extension(file.display_path())))
    }
}

/// A path's extension as the filter knows it: lowercase, without the dot;
/// `""` for none (and for dotfiles such as `.gitignore`).
pub fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) => name[i + 1..].to_ascii_lowercase(),
    }
}

/// Every extension among `files` with its file count, most common first
/// (then by name).
pub fn extensions(files: &[FileChange]) -> Vec<(String, usize)> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for f in files {
        *counts.entry(extension(f.display_path())).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// A fuzzy path query (`nucleo-matcher`, path mode: `/` is a word
/// boundary, smart case).
pub struct Fuzzy {
    matcher: Matcher,
    pattern: Pattern,
    buf: Vec<char>,
}

impl Fuzzy {
    /// `None` for a blank query (everything matches).
    pub fn new(query: &str) -> Option<Fuzzy> {
        if query.trim().is_empty() {
            return None;
        }
        Some(Fuzzy {
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            pattern: Pattern::parse(query, CaseMatching::Smart, Normalization::Smart),
            buf: Vec::new(),
        })
    }

    /// `path`'s score, `None` when it does not match.
    pub fn score(&mut self, path: &str) -> Option<u32> {
        self.pattern
            .score(Utf32Str::new(path, &mut self.buf), &mut self.matcher)
    }

    /// The matched characters' indices in `path` (sorted, deduplicated),
    /// `None` when it does not match.
    pub fn indices(&mut self, path: &str) -> Option<Vec<u32>> {
        let mut indices = Vec::new();
        self.pattern.indices(
            Utf32Str::new(path, &mut self.buf),
            &mut self.matcher,
            &mut indices,
        )?;
        indices.sort_unstable();
        indices.dedup();
        Some(indices)
    }
}

/// A filter menu item's effect.
type FilterToggle = Box<dyn Fn(&mut FileTree, &mut Context<FileTree>)>;

/// The filter menu of `tree` (the funnel button's, and `f`'s): Unviewed,
/// Has comments, the statuses and, with more than one, the extensions, each
/// checked when on; "Clear filters" while any is on.
/// `f` and `extensions` are the tree's filters and file extensions as the
/// menu opens.
pub(super) fn menu(
    tree: &WeakEntity<FileTree>,
    f: TreeFilter,
    extensions: Vec<(String, usize)>,
    mut menu: PopupMenu,
) -> PopupMenu {
    let item = |label: &str, checked: bool, act: FilterToggle| {
        let tree = tree.clone();
        PopupMenuItem::new(label.to_owned())
            .checked(checked)
            .on_click(move |_, _, cx| {
                if let Some(tree) = tree.upgrade() {
                    tree.update(cx, |t, cx| act(t, cx));
                }
            })
    };
    menu = menu
        .item(item(
            "Unviewed",
            f.unviewed,
            Box::new(|t, cx| t.toggle_unviewed(cx)),
        ))
        .item(item(
            "Has comments",
            f.has_comments,
            Box::new(|t, cx| t.toggle_has_comments(cx)),
        ))
        .separator()
        .label("Status");
    for status in StatusFilter::ALL {
        menu = menu.item(item(
            status.label(),
            f.statuses.contains(&status),
            Box::new(move |t, cx| t.toggle_status(status, cx)),
        ));
    }
    if extensions.len() > 1 {
        menu = menu.separator().label("Extension");
        for (ext, count) in &extensions {
            let label = if ext.is_empty() {
                format!("No extension ({count})")
            } else {
                format!(".{ext} ({count})")
            };
            let e = ext.clone();
            menu = menu.item(item(
                &label,
                f.extensions.contains(ext),
                Box::new(move |t, cx| t.toggle_extension(&e, cx)),
            ));
        }
    }
    if f.menu_active() {
        let tree = tree.clone();
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Clear filters").on_click(move |_, window, cx| {
                    if let Some(tree) = tree.upgrade() {
                        tree.update(cx, |t, cx| t.clear_filters(window, cx));
                    }
                }),
            );
    }
    menu.max_h(px(layout::OVERLAY_MAX_H)).scrollable(true)
}
