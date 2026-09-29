//! Every action of the app (design §11.9, ADR-0025): the §11.9 table, the
//! provisional macOS additions, and the palette-only actions later features
//! handle. One registry feeds the default bindings ([`super::defaults`]),
//! `keymap.json` ([`super::file`]), the command palette and the cheat sheet.
//!
//! Actions live in one module per namespace (`viewport`, `tree`,
//! `composer`, `tab`, `window`), so `keymap::actions::viewport::CursorDown`
//! is the action `keymap.json` names `"viewport::CursorDown"`. Features
//! handle them with [`super::handlers::on_action`] (scoped to a review tab or
//! the main window) or `cx.on_action` (app-wide); an action nobody handles
//! does nothing.

use gpui_kit::Action;

/// Actions of the diff viewport (key context `Viewport`).
pub mod viewport {
    gpui_kit::actions!(
        viewport,
        [
            /// `j` / `↓`: the line cursor to the next row (T3.8).
            CursorDown,
            /// `k` / `↑`: the line cursor to the previous row (T3.8).
            CursorUp,
            /// `⇧↓`: extend the line range down, on one side (T3.8).
            ExtendSelectionDown,
            /// `⇧↑`: extend the line range up, on one side (T3.8).
            ExtendSelectionUp,
            /// `n`: the next file (T3.8).
            NextFile,
            /// `p`: the previous file (T3.8).
            PrevFile,
            /// `]`: the next change (T3.8).
            NextChange,
            /// `[`: the previous change (T3.8).
            PrevChange,
            /// `v`: toggle Viewed, then collapse and jump to the next unviewed
            /// file (T3.7).
            ToggleViewed,
            /// `c`: comment on the cursor line or selection (T3.8 asks, T3.10
            /// opens the composer).
            Comment,
            /// `.`: the next open thread (T3.9).
            NextOpenThread,
            /// `,`: the previous open thread (T3.9).
            PrevOpenThread,
            /// `e`: expand the gap nearest the cursor by 20 lines (T3.8).
            ExpandContext,
            /// `E`: expand the cursor's whole file (T3.8).
            ExpandFile,
            /// `s`: switch between split and unified, remembered per diff.
            ToggleLayout,
            /// Split view (a manual choice, remembered per diff).
            LayoutSplit,
            /// Unified view (a manual choice, remembered per diff).
            LayoutUnified,
            /// Back to the automatic layout (split when the viewport is wide).
            LayoutAuto,
            /// `w`: hide or show whitespace-only changes.
            ToggleWhitespace,
            /// Word diff by words.
            WordDiffWord,
            /// Word diff by characters.
            WordDiffChar,
            /// No word diff highlights.
            WordDiffOff,
            /// `o`: open the cursor line in the editor (T3.16).
            OpenInEditor,
            /// `⌘C`: copy the selected source text (T3.8, provisional).
            Copy,
        ]
    );
}

/// Actions of the file tree (key context `Tree`).
pub mod tree {
    gpui_kit::actions!(
        tree,
        [
            /// `n`: the next file (T3.6).
            NextFile,
            /// `p`: the previous file (T3.6).
            PrevFile,
            /// `v`: toggle Viewed on the selected file (T3.7).
            ToggleViewed,
            /// Mark every file of the selected folder viewed (T3.7).
            MarkFolderViewed,
        ]
    );
}

/// Actions of the comment composer (key context `Composer`).
pub mod composer {
    gpui_kit::actions!(
        composer,
        [
            /// `⌘⏎`: save the draft (T3.10).
            SaveDraft,
            /// `Esc`: cancel the composer (T3.10).
            Cancel,
            /// Switch between Write and Preview (T3.10).
            TogglePreview,
        ]
    );
}

/// Actions of a review tab (key context `Tab`).
pub mod tab {
    pub use crate::review_tab::panes::ToggleThreadsPanel;

    gpui_kit::actions!(
        tab,
        [
            /// `R`: apply the banner's changes (T3.11).
            Refresh,
            /// Pin the live state as an iteration (T3.11).
            Snapshot,
            /// Pick the live diff's base (T3.11).
            ChooseBase,
            /// `⌘⇧⏎`: open the Submit review dialog (T3.10).
            SubmitReview,
            /// Comment on the file at the cursor (T3.10).
            CommentOnFile,
            /// Comment on the whole review (T3.10).
            CommentOnReview,
            /// Reassign the review to another agent session (T3.4, OQ-32).
            AssignToSession,
            /// `⌘F`: find across all files (T3.15).
            Find,
            /// Hide or show agent notes (T3.9).
            ToggleAgentNotes,
            /// Show the changes since the last review (T3.12).
            ToggleChangesSinceLastReview,
            /// Jump to the next unread agent reply (T3.13).
            NextUnreadThread,
        ]
    );
}

/// Actions of the main window (key context `Window`, or none).
pub mod window {
    pub use crate::tabs::{CloseTab, NextTab, PrevTab};
    pub use crate::window::{Minimize, Quit, Zoom};

    gpui_kit::actions!(
        window,
        [
            /// `⌘K`: the command palette.
            CommandPalette,
            /// `?`: the keyboard shortcuts cheat sheet.
            CheatSheet,
            /// `⌘P`: the file finder (T3.6).
            FileFinder,
            /// `⌘O`: open a review (T3.5).
            OpenFlow,
            /// `⌘,`: open `settings.json` in the editor (T3.16).
            OpenSettings,
        ]
    );
}

/// One entry of the action registry.
pub struct ActionInfo {
    /// The name `keymap.json` uses (`viewport::CursorDown`).
    pub name: &'static str,
    /// What the palette and the cheat sheet show.
    pub title: &'static str,
    /// Builds the action.
    pub build: fn() -> Box<dyn Action>,
}

impl ActionInfo {
    /// The namespace (`viewport`, `tree`, `composer`, `tab`, `window`).
    pub fn namespace(&self) -> &'static str {
        self.name.split_once("::").map_or("", |(ns, _)| ns)
    }
}

macro_rules! registry {
    ($($ns:ident :: $ty:ident => $title:literal),* $(,)?) => {
        &[$(ActionInfo {
            name: concat!(stringify!($ns), "::", stringify!($ty)),
            title: $title,
            build: || Box::new($ns::$ty),
        }),*]
    };
}

/// Every action, grouped by namespace in palette order.
pub const ACTIONS: &[ActionInfo] = registry![
    viewport::CursorDown => "Move cursor down",
    viewport::CursorUp => "Move cursor up",
    viewport::ExtendSelectionDown => "Extend selection down",
    viewport::ExtendSelectionUp => "Extend selection up",
    viewport::NextFile => "Next file",
    viewport::PrevFile => "Previous file",
    viewport::NextChange => "Next change",
    viewport::PrevChange => "Previous change",
    viewport::ToggleViewed => "Toggle viewed",
    viewport::Comment => "Comment on line or selection",
    viewport::NextOpenThread => "Next open thread",
    viewport::PrevOpenThread => "Previous open thread",
    viewport::ExpandContext => "Expand context",
    viewport::ExpandFile => "Expand whole file",
    viewport::ToggleLayout => "Toggle split / unified",
    viewport::LayoutSplit => "Split view",
    viewport::LayoutUnified => "Unified view",
    viewport::LayoutAuto => "Automatic layout",
    viewport::ToggleWhitespace => "Toggle hide whitespace",
    viewport::WordDiffWord => "Word diff: words",
    viewport::WordDiffChar => "Word diff: characters",
    viewport::WordDiffOff => "Word diff: off",
    viewport::OpenInEditor => "Open in editor",
    viewport::Copy => "Copy selection",
    tree::NextFile => "Next file in tree",
    tree::PrevFile => "Previous file in tree",
    tree::ToggleViewed => "Toggle viewed in tree",
    tree::MarkFolderViewed => "Mark folder viewed",
    composer::SaveDraft => "Save draft",
    composer::Cancel => "Cancel comment",
    composer::TogglePreview => "Toggle comment preview",
    tab::Refresh => "Refresh",
    tab::Snapshot => "Snapshot",
    tab::ChooseBase => "Choose base",
    tab::SubmitReview => "Submit review",
    tab::CommentOnFile => "Comment on file",
    tab::CommentOnReview => "Comment on review",
    tab::AssignToSession => "Assign to session",
    tab::Find => "Find in all files",
    tab::ToggleAgentNotes => "Hide agent notes",
    tab::ToggleChangesSinceLastReview => "Changes since last review",
    tab::NextUnreadThread => "Next unread reply",
    tab::ToggleThreadsPanel => "Toggle threads panel",
    window::CommandPalette => "Command palette",
    window::CheatSheet => "Keyboard shortcuts",
    window::FileFinder => "Go to file",
    window::OpenFlow => "Open review",
    window::OpenSettings => "Open settings",
    window::CloseTab => "Close tab",
    window::NextTab => "Next tab",
    window::PrevTab => "Previous tab",
    window::Minimize => "Minimize",
    window::Zoom => "Zoom",
    window::Quit => "Quit Polygloss",
];

/// The registry entry named `name` (`viewport::CursorDown`).
pub fn find(name: &str) -> Option<&'static ActionInfo> {
    ACTIONS.iter().find(|a| a.name == name)
}

/// The registry entry of `action`.
pub fn info_of(action: &dyn Action) -> Option<&'static ActionInfo> {
    find(action.name())
}
