//! The default key bindings: design §11.9 as data, plus its provisional
//! macOS additions. Keys use GPUI's spelling (`shift-e` is `E`, `cmd-}` is
//! ⌘⇧]); an empty context binds everywhere. `keymap.json` overrides them
//! ([`super::file`]).
//!
//! Single printable keys (`j`, `?`, `R`) never fire while a text field or a
//! composer has focus: [`super::gpui_predicate`] adds `!Input && !Composer`
//! to their context (ADR-0025).

/// `(keys, action, context)`.
pub type DefaultBinding = (&'static str, &'static str, &'static str);

/// Every default binding, in cheat-sheet order.
pub const DEFAULT_BINDINGS: &[DefaultBinding] = &[
    // Design §11.9.
    ("j", "viewport::CursorDown", "Viewport"),
    ("down", "viewport::CursorDown", "Viewport"),
    ("k", "viewport::CursorUp", "Viewport"),
    ("up", "viewport::CursorUp", "Viewport"),
    ("shift-down", "viewport::ExtendSelectionDown", "Viewport"),
    ("shift-up", "viewport::ExtendSelectionUp", "Viewport"),
    ("n", "viewport::NextFile", "Viewport"),
    ("p", "viewport::PrevFile", "Viewport"),
    ("n", "tree::NextFile", "Tree"),
    ("p", "tree::PrevFile", "Tree"),
    ("]", "viewport::NextChange", "Viewport"),
    ("[", "viewport::PrevChange", "Viewport"),
    ("v", "viewport::ToggleViewed", "Viewport"),
    ("v", "tree::ToggleViewed", "Tree"),
    ("c", "viewport::Comment", "Viewport"),
    ("cmd-enter", "composer::SaveDraft", "Composer"),
    (".", "viewport::NextOpenThread", "Viewport"),
    (",", "viewport::PrevOpenThread", "Viewport"),
    ("e", "viewport::ExpandContext", "Viewport"),
    ("shift-e", "viewport::ExpandFile", "Viewport"),
    ("s", "viewport::ToggleLayout", "Viewport"),
    ("w", "viewport::ToggleWhitespace", "Viewport"),
    ("shift-r", "tab::Refresh", "Tab"),
    ("o", "viewport::OpenInEditor", "Viewport"),
    ("cmd-p", "window::FileFinder", "Window"),
    ("cmd-k", "window::CommandPalette", "Window"),
    ("cmd-o", "window::OpenFlow", "Window"),
    ("cmd-f", "tab::Find", "Tab"),
    ("cmd-shift-enter", "tab::SubmitReview", "Tab"),
    ("?", "window::CheatSheet", "Window"),
    // Provisional macOS additions (§11.9). Popovers and dialogs close on
    // Esc by themselves (gpui-kit).
    ("escape", "composer::Cancel", "Composer"),
    ("cmd-c", "viewport::Copy", "Viewport"),
    ("cmd-w", "window::CloseTab", ""),
    ("cmd-}", "window::NextTab", ""),
    ("cmd-{", "window::PrevTab", ""),
    ("ctrl-tab", "window::NextTab", ""),
    ("ctrl-shift-tab", "window::PrevTab", ""),
    ("cmd-,", "window::OpenSettings", "Window"),
    ("cmd-q", "window::Quit", ""),
    ("cmd-m", "window::Minimize", ""),
    // Keyboard-only use (T5.6, OQ-23): Tab cycles the review tab's panes;
    // the file menu, collapse, the tree's filters, the iteration menu and
    // the threads panel, which only the mouse reached before.
    ("tab", "tab::FocusNextPane", "Tab"),
    ("shift-tab", "tab::FocusPrevPane", "Tab"),
    ("i", "tab::ChooseIteration", "Tab"),
    ("m", "viewport::FileMenu", "Viewport"),
    ("z", "viewport::ToggleCollapse", "Viewport"),
    ("/", "tree::FocusFilter", "Tree"),
    ("f", "tree::FilterMenu", "Tree"),
    ("j", "threads::SelectNext", "ThreadsPanel"),
    ("down", "threads::SelectNext", "ThreadsPanel"),
    ("k", "threads::SelectPrev", "ThreadsPanel"),
    ("up", "threads::SelectPrev", "ThreadsPanel"),
    ("enter", "threads::Open", "ThreadsPanel"),
    ("r", "threads::Reply", "ThreadsPanel"),
    ("x", "threads::ToggleResolved", "ThreadsPanel"),
    ("e", "threads::EditComment", "ThreadsPanel"),
];
