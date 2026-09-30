//! Line cursor, ranges, gutter "+", selection and copy: action wiring
//! (design §11.6, §11.9).
//!
//! The viewport owns the cursor, the "+" and mouse selection
//! (`polygloss_viewport::CursorPos`); this module maps the review tab's
//! `viewport::*` actions to it: `j`/`k`/`↓`/`↑` move the cursor, `⇧↓`/`⇧↑`
//! extend a range, `]`/`[` jump between changes, `n`/`p` between files, `c`
//! asks for a comment (`ViewportEvent::CommentRequested`, which the
//! composer opens, T3.10), `e` expands the gap nearest the cursor by 20 lines
//! toward it, `E` the cursor's whole file, and `⌘C` copies the selection as
//! source text. `m` opens the cursor file's ⋯ menu and `z` collapses or
//! expands the file (T5.6: the header's controls from the keyboard).
//!
//! Owned by T3.8. T3.1 already calls [`init`] (from `features::init`) and
//! [`attach`] (for every new review tab).

use gpui_kit::{App, Context, Window};
use polygloss_viewport::{DiffViewport, Direction};

use crate::keymap::actions::viewport;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;

/// Registers the cursor actions' handlers on review tabs.
pub fn init(cx: &mut App) {
    on(cx, |v, _: &viewport::CursorDown, cx| {
        v.move_cursor(Direction::Down, cx)
    });
    on(cx, |v, _: &viewport::CursorUp, cx| {
        v.move_cursor(Direction::Up, cx)
    });
    on(cx, |v, _: &viewport::ExtendSelectionDown, cx| {
        v.extend_selection(Direction::Down, cx)
    });
    on(cx, |v, _: &viewport::ExtendSelectionUp, cx| {
        v.extend_selection(Direction::Up, cx)
    });
    on(cx, |v, _: &viewport::NextChange, cx| v.next_change(cx));
    on(cx, |v, _: &viewport::PrevChange, cx| v.prev_change(cx));
    on(cx, |v, _: &viewport::NextFile, cx| v.next_file(cx));
    on(cx, |v, _: &viewport::PrevFile, cx| v.prev_file(cx));
    on(cx, |v, _: &viewport::Comment, cx| v.request_comment(cx));
    on(cx, |v, _: &viewport::ExpandContext, cx| {
        v.expand_context(cx)
    });
    on(cx, |v, _: &viewport::ExpandFile, cx| {
        v.expand_cursor_file(cx)
    });
    on(cx, |v, _: &viewport::Copy, cx| v.copy(cx));
    // The file header's controls from the keyboard (T5.6): `m` opens the
    // cursor file's ⋯ menu, `z` collapses or expands it.
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::FileMenu, window, cx| {
            tab.viewport.update(cx, |v, cx| {
                let f = v.current_file();
                v.open_file_menu(f, window, cx);
            });
        },
    );
    on(cx, |v, _: &viewport::ToggleCollapse, cx| {
        let f = v.current_file();
        v.toggle_collapsed(f, cx);
    });
}

/// Nothing per tab: the cursor lives in the tab's viewport.
pub fn attach(_tab: &mut ReviewTab, _window: &mut Window, _cx: &mut Context<ReviewTab>) {}

/// Handles action `A` in review tabs by running `f` on the tab's viewport.
fn on<A: gpui_kit::Action>(
    cx: &mut App,
    f: impl Fn(&mut DiffViewport, &A, &mut Context<DiffViewport>) + 'static,
) {
    handlers::on_action(cx, move |tab: &mut ReviewTab, action: &A, _, cx| {
        tab.viewport.update(cx, |v, cx| f(v, action, cx));
    });
}
