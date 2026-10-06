//! Themes and fonts (design §11.10, ADR-0024, ADR-0027).
//!
//! - [`ThemeRegistry`] (a global): Polygloss Light/Dark and Pierre
//!   Light/Dark built in, plus every Zed theme file in
//!   `~/.config/polygloss/themes/`, hot-reloaded; broken files are skipped
//!   with a toast.
//! - [`apply_theme`]: one Zed theme drives everything: its `style` colors
//!   become gpui-kit's theme tokens ([`zed_to_kit`]), its `syntax` the
//!   highlighter's `SyntaxTheme` and, with created/deleted/modified, the
//!   diff viewport's colors ([`viewport_theme`]).
//! - Which theme: `theme.mode` (`system` follows the macOS appearance, the
//!   main window reports changes), `theme.light` / `theme.dark` by name
//!   (Polygloss by default); a name that no theme has falls back to Polygloss
//!   of that appearance.
//! - [`fonts`]: the bundled Lilex, the default code font. The UI keeps the
//!   system font.

pub mod fonts;
pub mod registry;
pub mod viewport_theme;
pub mod zed_to_kit;

use std::sync::Arc;

use gpui_kit::component::theme::Theme;
use gpui_kit::{App, Global, SharedString, WindowAppearance};
use polygloss_highlight::{Appearance, ZedTheme, default_theme};
use polygloss_viewport::ViewportTheme;

pub use registry::{THEMES_DIR, ThemeFileError, ThemeRegistry};

use crate::app_state::AppState;
use crate::settings::SettingsStore;
use crate::settings::model::ThemeMode as ModeSetting;
use crate::window::MainWindow;

/// The theme in effect (a GPUI global).
pub struct ActiveTheme {
    pub zed: Arc<ZedTheme>,
    /// The viewport's colors, shared by every review tab.
    pub viewport: Arc<ViewportTheme>,
    /// The macOS appearance last reported (for `theme.mode = "system"`).
    pub system_appearance: Appearance,
    /// A missing theme name already reported (reported once per name).
    missing_reported: Option<String>,
}

impl Global for ActiveTheme {}

impl ActiveTheme {
    pub fn global(cx: &App) -> &ActiveTheme {
        cx.global::<ActiveTheme>()
    }

    pub fn name(&self) -> SharedString {
        self.zed.name.clone().into()
    }
}

/// Loads the themes, applies the one the settings ask for, and keeps it in
/// step with the settings, the theme files and the system appearance.
/// `startup::init` has registered the fonts ([`fonts::register_fonts`]) and
/// initialized gpui-kit and the settings before.
pub fn init(cx: &mut App) {
    let dir = AppState::global(cx).paths.config_dir.join(THEMES_DIR);
    ThemeRegistry::init_at(&dir, cx, sync);
    let system_appearance = appearance(cx.window_appearance());
    let initial = default_theme(system_appearance);
    cx.set_global(ActiveTheme {
        zed: Arc::new(initial.clone()),
        viewport: viewport_theme::resolve(initial),
        system_appearance,
        missing_reported: None,
    });
    // Apply fully once, whatever the placeholder above was.
    apply(resolve_theme(cx), true, cx);
    cx.observe_global::<SettingsStore>(sync).detach();
    cx.observe_new::<MainWindow>(|main, window, cx| {
        let Some(window) = window else { return };
        // The window's own observer (not the view's): the callback must
        // not hold `MainWindow`, since a theme change reads its tabs.
        window
            .observe_window_appearance(|window, cx| {
                system_appearance_changed(appearance(window.appearance()), cx)
            })
            .detach();
        system_appearance_changed(appearance(window.appearance()), cx);
        for message in cx.global_mut::<ThemeRegistry>().take_unannounced() {
            main.toast_error(message, window, cx);
        }
    })
    .detach();
}

/// The Zed appearance of a macOS one.
pub fn appearance(a: WindowAppearance) -> Appearance {
    match a {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
    }
}

/// The diff viewport's theme for new review tabs: the active theme's.
pub fn viewport_theme(cx: &App) -> Arc<ViewportTheme> {
    match cx.try_global::<ActiveTheme>() {
        Some(active) => active.viewport.clone(),
        None => Arc::new(ViewportTheme::from_zed(default_theme(Appearance::Light))),
    }
}

/// The active theme's name.
pub fn active_theme_name(cx: &App) -> SharedString {
    ActiveTheme::global(cx).name()
}

/// Makes the theme called `name` active: gpui-kit's tokens, the syntax and
/// diff colors of every open viewport. `false` if no theme has that name
/// (nothing changes). The next settings or appearance change picks the
/// theme from the settings again.
pub fn apply_theme(name: &str, cx: &mut App) -> bool {
    match ThemeRegistry::global(cx).get(name) {
        Some(theme) => {
            apply(theme, false, cx);
            true
        }
        None => false,
    }
}

/// Scans the themes directory again and re-applies the theme the settings
/// ask for (what the directory watcher does); new broken files are toasted.
pub fn reload(cx: &mut App) {
    ThemeRegistry::reload(cx);
    sync(cx);
}

/// The macOS appearance changed (the main window observes it). With
/// `theme.mode = "system"` this switches between `theme.light` and
/// `theme.dark`.
pub fn system_appearance_changed(appearance: Appearance, cx: &mut App) {
    if ActiveTheme::global(cx).system_appearance == appearance {
        return;
    }
    cx.global_mut::<ActiveTheme>().system_appearance = appearance;
    sync(cx);
}

/// Re-applies the theme the settings and the appearance ask for, and the
/// code font as gpui-kit's monospace family.
fn sync(cx: &mut App) {
    let theme = resolve_theme(cx);
    apply(theme, false, cx);
    sync_kit_mono_font(cx);
    // Outside whatever window update this runs in (an appearance change).
    cx.defer(flush_announcements);
}

/// The theme the settings ask for now; Polygloss of the wanted appearance
/// when no theme has the configured name (reported once).
fn resolve_theme(cx: &mut App) -> Arc<ZedTheme> {
    let settings = SettingsStore::global(cx).shared();
    let system = ActiveTheme::global(cx).system_appearance;
    let wanted = match settings.theme.mode {
        ModeSetting::Light => Appearance::Light,
        ModeSetting::Dark => Appearance::Dark,
        ModeSetting::System => system,
    };
    let name = match wanted {
        Appearance::Light => &settings.theme.light,
        Appearance::Dark => &settings.theme.dark,
    };
    if let Some(theme) = ThemeRegistry::global(cx).get(name) {
        return theme;
    }
    let fallback = default_theme(wanted);
    let active = cx.global_mut::<ActiveTheme>();
    if active.missing_reported.as_deref() != Some(name.as_str()) {
        active.missing_reported = Some(name.clone());
        let message = format!("No theme named \"{name}\", using {}", fallback.name);
        tracing::warn!("{message}");
        cx.global_mut::<ThemeRegistry>().announce(message.into());
    }
    ThemeRegistry::global(cx)
        .get(&fallback.name)
        .unwrap_or_else(|| Arc::new(fallback.clone()))
}

/// Puts `theme` in effect unless it already is (`force`: always).
fn apply(theme: Arc<ZedTheme>, force: bool, cx: &mut App) {
    let active = ActiveTheme::global(cx);
    if !force && (Arc::ptr_eq(&active.zed, &theme) || *active.zed == *theme) {
        return;
    }
    let config = zed_to_kit::kit_theme_config(&theme);
    let viewport = viewport_theme::resolve(&theme);
    Theme::update(cx, |kit| kit.apply_config(&config));
    crate::motion::apply_kit_tokens(cx);
    let active = cx.global_mut::<ActiveTheme>();
    active.zed = theme;
    active.viewport = viewport.clone();
    viewport_theme::push_to_open_tabs(&viewport, cx);
}

/// Names the code font as gpui-kit's monospace family when it changes
/// (`polygloss_viewport::kit::kit_fonts`: the family if installed, never
/// Menlo, else SF Mono, which "SF Mono" and "System Mono" also name), as
/// `init_kit` did at startup.
fn sync_kit_mono_font(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let family = SettingsStore::global(cx)
        .settings()
        .buffer_font
        .family
        .clone();
    let installed = polygloss_viewport::kit::is_installed(cx.text_system(), &family);
    let mono = polygloss_viewport::kit::kit_fonts(&family, installed).mono;
    if Theme::global(cx).mono_font_family != mono {
        Theme::update(cx, |kit| kit.mono_font_family = mono);
    }
}

/// Shows queued theme messages in the main window, if it is open (else it
/// shows them when it opens).
fn flush_announcements(cx: &mut App) {
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    let messages = cx.global_mut::<ThemeRegistry>().take_unannounced();
    if messages.is_empty() {
        return;
    }
    let result = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| {
            for message in messages {
                main.toast_error(message, window, cx);
            }
        })
    });
    if let Err(e) = result {
        tracing::warn!("showing theme messages: {e:#}");
    }
}
