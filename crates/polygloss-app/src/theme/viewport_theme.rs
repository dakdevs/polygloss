//! The diff viewport's side of a theme: created/deleted rows and words,
//! `+`/`-` accents, headers and syntax, all resolved from the active Zed
//! theme by `polygloss_viewport::ViewportTheme::from_zed`, and pushed into
//! every open review tab when the theme changes.
//!
//! One `Arc<ViewportTheme>` per active theme: every tab shares it, so the
//! viewport's "did the theme change" check (`Arc::ptr_eq`) only fires on a
//! real change and highlighted tokens stay cached otherwise.

use std::sync::Arc;

use gpui_kit::App;
use polygloss_viewport::ViewportTheme;

use crate::tabs::TabItem;

/// Resolves the viewport colors of `t`.
pub fn resolve(t: &polygloss_highlight::ZedTheme) -> Arc<ViewportTheme> {
    Arc::new(ViewportTheme::from_zed(t))
}

/// Gives every open review tab's viewport `theme`, keeping its other options
/// (layout and view toggles stay as they are).
pub fn push_to_open_tabs(theme: &Arc<ViewportTheme>, cx: &mut App) {
    let Some((_, main)) = crate::window::main_window(cx) else {
        return;
    };
    let viewports: Vec<_> = main
        .read(cx)
        .tabs()
        .items()
        .iter()
        .filter_map(TabItem::review)
        .map(|tab| tab.read(cx).viewport.clone())
        .collect();
    for viewport in viewports {
        viewport.update(cx, |v, cx| {
            if Arc::ptr_eq(&v.options().theme, theme) {
                return;
            }
            let mut opts = v.options().clone();
            opts.theme = theme.clone();
            v.set_options(opts, cx);
        });
    }
}
