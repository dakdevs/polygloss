//! The app's spacing tokens (ADR-0031, design §11.17): every group of
//! [`polygloss_viewport::space`] (the viewport paints the canvas and the
//! cards, and the app's header card, thread blocks and composer mirror its
//! geometry), plus [`layout`], the app-only sizes, and [`TextStyleExt`],
//! which sets a [`text`] style's size and line height together.
//!
//! Call sites write module-qualified paths (`px(space::height::SM)`), never
//! gpui's rem helpers; `tests/scripts/spacing-tokens.test.ts` rejects raw
//! dimensions anywhere else.

pub use polygloss_viewport::space::*;

use gpui_kit::{Styled, px};

/// App-only sizes: top-row and panel widths, minimum widths, overlay and
/// dialog sizes, Home's columns, the toolbar's give-way caps, the tree's
/// Viewed slot, the divider's drag band and the first window size.
pub mod layout {
    /// The first traffic light's x in a unified toolbar, each light's size
    /// and the gap between them: the macOS 26 probe puts the three 14 pt
    /// lights at x = 19, 42 and 65. AppKit's, set in the window's options.
    pub const TRAFFIC_LIGHT_X: f32 = 19.0;
    pub const TRAFFIC_LIGHT: f32 = 14.0;
    pub const TRAFFIC_LIGHT_GAP: f32 = 9.0;
    /// Where the zoom button ends.
    pub const TRAFFIC_LIGHTS_END: f32 =
        TRAFFIC_LIGHT_X + 3.0 * TRAFFIC_LIGHT + 2.0 * TRAFFIC_LIGHT_GAP;
    /// The toolbar's leading inset while the sidebar is hidden: one light
    /// gap after the zoom button. The show-sidebar button sits there.
    pub const TOOLBAR_INSET_HIDDEN: f32 = TRAFFIC_LIGHTS_END + TRAFFIC_LIGHT_GAP;
    pub const SIDEBAR_WIDTH: f32 = 280.0;
    /// The sidebar's drag-resize range.
    pub const SIDEBAR_RANGE: (f32, f32) = (220.0, 480.0);
    /// The main column's minimum beside the sidebar.
    pub const MAIN_MIN_WIDTH: f32 = 320.0;
    pub const THREADS_WIDTH: f32 = 340.0;
    /// The threads panel's drag-resize range.
    pub const THREADS_RANGE: (f32, f32) = (220.0, 720.0);
    /// The main column's minimum while the threads panel shows.
    pub const THREADS_MAIN_MIN_WIDTH: f32 = 480.0;
    /// The diff viewport's minimum beside the threads panel.
    pub const VIEWPORT_MIN_WIDTH: f32 = 260.0;
    /// The palette, the finder and the pickers: width and offset from the
    /// window's top.
    pub const PICKER_W: f32 = 560.0;
    pub const PICKER_TOP: f32 = 96.0;
    /// The open flow's and the cheat sheet's offset from the window's top.
    pub const DIALOG_TOP: f32 = 72.0;
    pub const DIALOG_W: f32 = 520.0;
    pub const MENU_MIN_W: f32 = 280.0;
    /// Every overlay list's maximum height.
    pub const OVERLAY_MAX_H: f32 = 400.0;
    /// The open flow's width and height.
    pub const OPEN_FLOW: (f32, f32) = (640.0, 460.0);
    pub const CHEAT_SHEET_W: f32 = 1040.0;
    /// A Home card's height.
    pub const HOME_CARD: f32 = 60.0;
    pub const HOME_MAX_W: f32 = 1120.0;
    /// Home's columns: the kind badge, Viewed, threads, the agent, the time.
    pub const HOME_COL_KIND: f32 = 68.0;
    pub const HOME_COL_VIEWED: f32 = 96.0;
    pub const HOME_COL_THREADS: f32 = 104.0;
    pub const HOME_COL_AGENT: f32 = 112.0;
    pub const HOME_COL_UPDATED: f32 = 72.0;
    pub const COMPOSER_MIN_H: f32 = 64.0;
    /// Find results' line-number column.
    pub const FIND_NUMBER_COL: f32 = 40.0;
    /// The toolbar's give-way caps: a pill's text and the repo name.
    pub const PILL_TEXT_MAX: f32 = 64.0;
    pub const REPO_NAME_MAX: f32 = 48.0;
    /// The tree's reserved Viewed slot (OQ-43).
    pub const VIEWED_SLOT: f32 = 20.0;
    /// gpui-kit's split handle band, which takes drags beside the divider.
    pub const DIVIDER_DRAG: f32 = 9.0;
    /// The main window's first size.
    pub const WINDOW: (f32, f32) = (1440.0, 900.0);
    /// The smallest window: it holds the narrowest sidebar beside the main
    /// column's minimum with the threads panel showing.
    pub const WINDOW_MIN: (f32, f32) = (720.0, 480.0);

    /// Every token with its values (a pair's two in order).
    pub const ALL: &[(&str, &[f32])] = &[
        ("TRAFFIC_LIGHT_X", &[TRAFFIC_LIGHT_X]),
        ("TRAFFIC_LIGHT", &[TRAFFIC_LIGHT]),
        ("TRAFFIC_LIGHT_GAP", &[TRAFFIC_LIGHT_GAP]),
        ("TRAFFIC_LIGHTS_END", &[TRAFFIC_LIGHTS_END]),
        ("TOOLBAR_INSET_HIDDEN", &[TOOLBAR_INSET_HIDDEN]),
        ("SIDEBAR_WIDTH", &[SIDEBAR_WIDTH]),
        ("SIDEBAR_RANGE", &[SIDEBAR_RANGE.0, SIDEBAR_RANGE.1]),
        ("MAIN_MIN_WIDTH", &[MAIN_MIN_WIDTH]),
        ("THREADS_WIDTH", &[THREADS_WIDTH]),
        ("THREADS_RANGE", &[THREADS_RANGE.0, THREADS_RANGE.1]),
        ("THREADS_MAIN_MIN_WIDTH", &[THREADS_MAIN_MIN_WIDTH]),
        ("VIEWPORT_MIN_WIDTH", &[VIEWPORT_MIN_WIDTH]),
        ("PICKER_W", &[PICKER_W]),
        ("PICKER_TOP", &[PICKER_TOP]),
        ("DIALOG_TOP", &[DIALOG_TOP]),
        ("DIALOG_W", &[DIALOG_W]),
        ("MENU_MIN_W", &[MENU_MIN_W]),
        ("OVERLAY_MAX_H", &[OVERLAY_MAX_H]),
        ("OPEN_FLOW", &[OPEN_FLOW.0, OPEN_FLOW.1]),
        ("CHEAT_SHEET_W", &[CHEAT_SHEET_W]),
        ("HOME_CARD", &[HOME_CARD]),
        ("HOME_MAX_W", &[HOME_MAX_W]),
        ("HOME_COL_KIND", &[HOME_COL_KIND]),
        ("HOME_COL_VIEWED", &[HOME_COL_VIEWED]),
        ("HOME_COL_THREADS", &[HOME_COL_THREADS]),
        ("HOME_COL_AGENT", &[HOME_COL_AGENT]),
        ("HOME_COL_UPDATED", &[HOME_COL_UPDATED]),
        ("COMPOSER_MIN_H", &[COMPOSER_MIN_H]),
        ("FIND_NUMBER_COL", &[FIND_NUMBER_COL]),
        ("PILL_TEXT_MAX", &[PILL_TEXT_MAX]),
        ("REPO_NAME_MAX", &[REPO_NAME_MAX]),
        ("VIEWED_SLOT", &[VIEWED_SLOT]),
        ("DIVIDER_DRAG", &[DIVIDER_DRAG]),
        ("WINDOW", &[WINDOW.0, WINDOW.1]),
        ("WINDOW_MIN", &[WINDOW_MIN.0, WINDOW_MIN.1]),
    ];
}

/// Sets a [`text`] style: its size and its line height together. The only
/// place UI code sets a line height.
pub trait TextStyleExt: Styled {
    fn text_style(self, style: text::Style) -> Self {
        let (size, line_height) = style;
        self.text_size(px(size)).line_height(px(line_height))
    }
}

impl<E: Styled> TextStyleExt for E {}
