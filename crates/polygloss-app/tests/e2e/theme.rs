//! Screenshots of T3.3 and T6.2 (design §11.10): the whole app in the
//! default Polygloss Light (split) and Polygloss Dark (unified), and in Pierre
//! Light and Dark picked by name; chrome and diff from one theme, code in the
//! bundled Lilex. Plus Lilex on the real macOS text system.

use std::sync::Arc;

use gpui_kit::component::theme::Theme;
use gpui_kit::{HeadlessAppContext, font, px, size};
use polygloss_app::review_tab::open_review;
use polygloss_app::settings::model::{LayoutSetting, ThemeMode};
use polygloss_app::settings::{Settings, SettingsStore};
use polygloss_app::tabs::TabItem;
use polygloss_app::theme::fonts::LILEX_FAMILY;
use polygloss_app::{startup, theme, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_viewport::kit::is_installed;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{Sandbox, code_change_repo};

pub const TESTS: &[Test] = &crate::tests![
    e2e_lilex_resolves_on_the_real_text_system,
    e2e_theme_pierre_light_split,
    e2e_theme_pierre_dark_unified,
    e2e_theme_polygloss_light_split,
    e2e_theme_polygloss_dark_unified,
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 20;

fn e2e_lilex_resolves_on_the_real_text_system() {
    let _sb = Sandbox::isolate();
    // `headless_app` registers the bundled faces, as `startup::init` does.
    let mut cx = screenshot::headless_app();
    cx.update(|cx| {
        let text_system = cx.text_system();
        assert!(is_installed(text_system, LILEX_FAMILY), "Lilex resolves");
        // Bold and italic resolve to Lilex faces, not a synthesized fallback.
        for f in [font(LILEX_FAMILY).bold(), font(LILEX_FAMILY).italic()] {
            let id = text_system.resolve_font(&f);
            let resolved = text_system.get_font_for_id(id).expect("a font");
            assert_eq!(resolved.family.as_ref(), LILEX_FAMILY, "{f:?}");
        }
        // gpui-kit's monospace family becomes Lilex through init_kit.
        polygloss_viewport::kit::init_kit(LILEX_FAMILY, cx);
        assert_eq!(Theme::global(cx).mono_font_family.as_ref(), LILEX_FAMILY);
    });
}

fn e2e_theme_pierre_light_split() {
    capture_app(
        ThemeMode::Light,
        Some("Pierre Light"),
        LayoutSetting::Split,
        false,
    );
}

fn e2e_theme_pierre_dark_unified() {
    capture_app(
        ThemeMode::Dark,
        Some("Pierre Dark"),
        LayoutSetting::Unified,
        true,
    );
}

fn e2e_theme_polygloss_light_split() {
    capture_app(ThemeMode::Light, None, LayoutSetting::Split, false);
}

fn e2e_theme_polygloss_dark_unified() {
    capture_app(ThemeMode::Dark, None, LayoutSetting::Unified, true);
}

/// Opens the app's main window at 1280×800 on the code-change fixture with
/// `mode`, the theme `named` for it (`None`: the default, Polygloss) and
/// `layout` pinned, waits for the tab to settle and compares it with the
/// test's baseline.
fn capture_app(mode: ThemeMode, named: Option<&str>, layout: LayoutSetting, threads_panel: bool) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    // The settings file the app reads at startup (the headless context runs
    // no effect cycle between updates, so observers of a later change would
    // not run before the capture).
    let mut settings = Settings::default();
    settings.theme.mode = mode;
    settings.diff.layout = layout;
    if let Some(name) = named {
        match mode {
            ThemeMode::Dark => settings.theme.dark = name.into(),
            _ => settings.theme.light = name.into(),
        }
    }
    let expected = named.unwrap_or(match mode {
        ThemeMode::Dark => "Polygloss Dark",
        _ => "Polygloss Light",
    });
    let file = sb.config_dir().join("polygloss/settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_string(&settings).unwrap()).unwrap();
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core, cx);
        assert_eq!(*SettingsStore::global(cx).settings(), settings);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    assert_eq!(cx.update(|cx| theme::active_theme_name(cx)), expected);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    let tab = settle(&mut cx, handle, &main);
    cx.update(|cx| tab.update(cx, |t, cx| t.set_threads_panel_visible(threads_panel, cx)));
    for _ in 0..3 {
        screenshot::draw(&mut cx, handle);
    }
    let (name, code_font) = cx.update(|cx| {
        let v = tab.read(cx).viewport.read(cx);
        (
            v.options().theme.name.clone(),
            v.options().code_font.clone(),
        )
    });
    assert_eq!(
        (name.as_str(), code_font.as_ref()),
        (expected, LILEX_FAMILY)
    );
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

/// Draws until the review tab shows highlighted rows; returns the tab.
fn settle(
    cx: &mut HeadlessAppContext,
    handle: gpui_kit::AnyWindowHandle,
    main: &gpui_kit::Entity<polygloss_app::window::MainWindow>,
) -> gpui_kit::Entity<polygloss_app::review_tab::ReviewTab> {
    for _ in 0..MAX_FRAMES {
        screenshot::draw(cx, handle);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        if let Some(tab) = tab {
            let viewport = cx.update(|cx| tab.read(cx).viewport.clone());
            let debug = cx.update(|cx| viewport.read(cx).debug());
            if debug.visible_rows.len() > 10
                && !debug.visible_rows.iter().any(|r| r == "Loading…")
                && debug.styled_rows > 0
            {
                return tab;
            }
        }
    }
    panic!("the review tab never settled");
}
