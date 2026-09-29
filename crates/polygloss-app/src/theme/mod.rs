//! Themes and fonts: Pierre Light/Dark, user Zed themes, Lilex (design
//! §11.10).
//!
//! Owned by T3.3. Created as a stub by T3.1, which already calls [`init`]
//! (from `features::init`) and takes every review tab's diff colors from
//! [`viewport_theme`].

use std::sync::Arc;

use gpui_kit::App;
use polygloss_highlight::Appearance;
use polygloss_viewport::ViewportTheme;

/// Registers the theme registry and appearance tracking.
pub fn init(_cx: &mut App) {}

/// The diff viewport's theme for new review tabs. Until T3.3 this is Pierre
/// Light whatever `theme.mode` says.
pub fn viewport_theme(_cx: &App) -> Arc<ViewportTheme> {
    Arc::new(ViewportTheme::pierre(Appearance::Light))
}
