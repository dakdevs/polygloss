//! Themes and fonts (T3.3, design §11.10, ADR-0024): Pierre → gpui-kit
//! tokens, system appearance, user theme files, broken files, Lilex.

use std::time::{Duration, Instant};

use gpui_kit::component::theme::Theme;
use gpui_kit::{Hsla, TestAppContext, VisualTestContext};
use polygloss_app::settings::model::ThemeMode;
use polygloss_app::settings::{Settings, SettingsStore};
use polygloss_app::theme::fonts::{LILEX_FAMILY, LILEX_FONTS};
use polygloss_app::theme::zed_to_kit::{REQUIRED_KIT_TOKENS, kit_colors, kit_theme_config};
use polygloss_app::theme::{self, ThemeRegistry};
use polygloss_highlight::{Appearance, Rgba, pierre_theme};
use polygloss_viewport::kit::{SYSTEM_MONO_FONT, SYSTEM_UI_FONT};

use crate::shell::{compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

fn color(hex: &str) -> Hsla {
    gpui_kit::rgba(Rgba::parse(hex).expect("a color").to_u32()).into()
}

/// Runs the app until `done` holds (watchers report from their own thread),
/// for at most 10 s.
fn wait_until(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(cx) {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(20));
        cx.executor().advance_clock(Duration::from_millis(50));
        draw(cx);
    }
}

/// A Zed theme family file with one theme.
fn zed_theme_file(name: &str, appearance: &str, background: &str) -> String {
    serde_json::json!({
        "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
        "name": format!("{name} family"),
        "author": "Test",
        "themes": [{
            "name": name,
            "appearance": appearance,
            "style": {
                "editor.background": background,
                "text": "#e0e0e0ff",
                "border": "#303030ff",
                "created": "#00ff00ff",
                "deleted": "#ff0000ff",
                "syntax": { "keyword": { "color": "#ff00ffff", "font_weight": 700 } }
            }
        }]
    })
    .to_string()
}

#[gpui_kit::test]
fn pierre_light_maps_every_required_kit_token(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let pierre = pierre_theme(Appearance::Light);
    let colors = kit_colors(pierre);
    let missing: Vec<_> = REQUIRED_KIT_TOKENS
        .iter()
        .filter(|k| !colors.contains_key(**k))
        .collect();
    assert!(
        missing.is_empty(),
        "not mapped from Pierre Light: {missing:?}"
    );
    // Straight from Pierre's own keys.
    for (kit, zed) in [
        ("background", "#ffffffff"),           // editor.background
        ("foreground", "#0a0a0aff"),           // text
        ("border", "#e5e5e5ff"),               // border
        ("sidebar.background", "#f5f5f5ff"),   // panel.background
        ("muted.foreground", "#525252ff"),     // text.muted
        ("primary.background", "#009fffff"),   // text.accent
        ("primary.foreground", "#ffffffff"),   // contrast on the accent
        ("selection.background", "#009fff2e"), // players[0].selection
        ("tab.active.background", "#ffffffff"),
        ("base.green", "#18a46cff"),
        ("base.red", "#d52c36ff"),
    ] {
        assert_eq!(colors[kit], zed, "{kit}");
    }
    // Every value is a color the kit's schema reads, under a key it knows.
    let config = kit_theme_config(pierre);
    let written = serde_json::to_value(&config.colors).unwrap();
    for key in colors.keys() {
        assert!(
            written.get(key).is_some_and(|v| !v.is_null()),
            "gpui-kit dropped {key}"
        );
    }
    assert!(config.font_family.is_none() && config.mono_font_family.is_none());

    // Applied in the app: gpui-kit's theme holds Pierre's colors, and the
    // fonts init_kit named stay (no font scan).
    let shell = start(cx);
    shell.cx.update(|_, cx| {
        assert_eq!(theme::active_theme_name(cx), "Pierre Light");
        let kit = Theme::global(cx);
        assert_eq!(kit.theme_name(), "Pierre Light");
        assert!(!kit.is_dark());
        assert_eq!(kit.background, color("#ffffff"));
        assert_eq!(kit.foreground, color("#0a0a0a"));
        assert_eq!(kit.border, color("#e5e5e5"));
        assert_eq!(kit.sidebar, color("#f5f5f5"));
        assert_eq!(kit.primary, color("#009fff"));
        assert_eq!(kit.green, color("#18a46c"));
        assert_eq!(kit.tokens.background.color, kit.background);
        assert_eq!(kit.font_family, SYSTEM_UI_FONT);
        assert_eq!(kit.mono_font_family, LILEX_FAMILY);
    });
}

#[gpui_kit::test]
fn system_appearance_switch_changes_theme(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let viewport_theme = |cx: &mut VisualTestContext| {
        viewport.read_with(cx, |v, _| {
            (v.options().theme.name.clone(), v.options().theme.appearance)
        })
    };
    // The test window is light: `theme.mode = "system"` picks Pierre Light.
    assert_eq!(
        viewport_theme(shell.cx),
        ("Pierre Light".into(), Appearance::Light)
    );

    shell
        .cx
        .update(|_, cx| theme::system_appearance_changed(Appearance::Dark, cx));
    draw(shell.cx);
    assert_eq!(
        viewport_theme(shell.cx),
        ("Pierre Dark".into(), Appearance::Dark)
    );
    shell.cx.update(|_, cx| {
        assert_eq!(theme::active_theme_name(cx), "Pierre Dark");
        let kit = Theme::global(cx);
        assert!(kit.is_dark());
        assert_eq!(kit.background, color("#0a0a0a"));
    });
    // New tabs get it too.
    assert_eq!(
        shell
            .cx
            .update(|_, cx| theme::viewport_theme(cx).name.clone()),
        "Pierre Dark"
    );

    // With the mode pinned to light, the system appearance no longer
    // matters.
    shell.cx.update(|_, cx| {
        let mut s = Settings::default();
        s.theme.mode = ThemeMode::Light;
        SettingsStore::set(s, cx);
    });
    draw(shell.cx);
    assert_eq!(viewport_theme(shell.cx).0, "Pierre Light");
    shell
        .cx
        .update(|_, cx| theme::system_appearance_changed(Appearance::Light, cx));
    shell
        .cx
        .update(|_, cx| theme::system_appearance_changed(Appearance::Dark, cx));
    draw(shell.cx);
    assert_eq!(viewport_theme(shell.cx).0, "Pierre Light");
    assert!(!shell.cx.update(|_, cx| Theme::global(cx).is_dark()));
}

#[gpui_kit::test]
fn user_theme_file_is_listed_and_applies(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let dir = sb.config_dir().join("polygloss/themes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("midnight.json"),
        zed_theme_file("Midnight", "dark", "#123456ff"),
    )
    .unwrap();
    let mut shell = start(cx);
    let names = shell.cx.update(|_, cx| ThemeRegistry::global(cx).names());
    for name in ["Midnight", "Pierre Dark", "Pierre Light"] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());

    // Selected in settings.json: the chrome, the diff and the syntax follow.
    std::fs::write(
        sb.config_dir().join("polygloss/settings.json"),
        r#"{ "theme": { "mode": "dark", "dark": "Midnight" } }"#,
    )
    .unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
    shell.cx.update(|_, cx| {
        assert_eq!(theme::active_theme_name(cx), "Midnight");
        let kit = Theme::global(cx);
        assert!(kit.is_dark());
        assert_eq!(kit.background, color("#123456"));
        assert_eq!(kit.border, color("#303030"));
    });
    viewport.read_with(shell.cx, |v, _| {
        let t = &v.options().theme;
        assert_eq!(t.name, "Midnight");
        assert_eq!(t.background, color("#123456"));
        assert_eq!(t.added_accent, color("#00ff00"));
        assert_eq!(t.removed_accent, color("#ff0000"));
        assert_eq!(
            t.syntax_id(),
            polygloss_highlight::SyntaxTheme::from_zed(&shell_theme("Midnight", &dir)).id()
        );
    });

    // Hot reload: editing the file while the app runs re-applies it.
    std::fs::write(
        dir.join("midnight.json"),
        zed_theme_file("Midnight", "dark", "#224466ff"),
    )
    .unwrap();
    wait_until(shell.cx, |cx| {
        viewport.read_with(cx, |v, _| v.options().theme.background == color("#224466"))
    });
    // …and a file dropped in later is listed.
    std::fs::write(
        dir.join("dawn.json"),
        zed_theme_file("Dawn", "light", "#fff8f0ff"),
    )
    .unwrap();
    wait_until(shell.cx, |cx| {
        cx.update(|_, cx| ThemeRegistry::global(cx).get("Dawn").is_some())
    });
    // apply_theme by name.
    assert!(shell.cx.update(|_, cx| theme::apply_theme("Dawn", cx)));
    draw(shell.cx);
    viewport.read_with(shell.cx, |v, _| assert_eq!(v.options().theme.name, "Dawn"));
    assert!(
        !shell
            .cx
            .update(|_, cx| theme::apply_theme("No Such Theme", cx))
    );
}

/// The first theme of the family file `<dir>/<name lowercased>.json`.
fn shell_theme(name: &str, dir: &std::path::Path) -> polygloss_highlight::ZedTheme {
    let text = std::fs::read_to_string(dir.join(format!("{}.json", name.to_lowercase()))).unwrap();
    polygloss_highlight::load_theme_family(&text)
        .unwrap()
        .themes
        .remove(0)
}

#[gpui_kit::test]
fn broken_user_theme_is_skipped_with_toast(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let dir = sb.config_dir().join("polygloss/themes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("broken.json"),
        r#"{ "name": "Broken", "themes": [ "#,
    )
    .unwrap();
    std::fs::write(
        dir.join("empty.json"),
        r#"{ "name": "Empty", "themes": [] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("good.json"),
        zed_theme_file("Good", "light", "#fafafaff"),
    )
    .unwrap();
    // Not a theme file: ignored without an error.
    std::fs::write(dir.join("notes.txt"), "not json").unwrap();
    let shell = start(cx);
    let (names, errors) = shell.cx.update(|_, cx| {
        let r = ThemeRegistry::global(cx);
        (r.names(), r.errors().to_vec())
    });
    assert!(names.iter().any(|n| n == "Good"), "{names:?}");
    assert!(
        !names.iter().any(|n| n == "Broken" || n == "Empty"),
        "{names:?}"
    );
    let failed: Vec<_> = errors
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(failed, ["broken.json", "empty.json"]);
    // Each is toasted once the window is up, naming the file.
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    for file in ["broken.json", "empty.json"] {
        assert_eq!(
            toasts.iter().filter(|t| t.contains(file)).count(),
            1,
            "{file}: {toasts:?}"
        );
    }
    // The app still shows its default theme.
    assert_eq!(
        shell.cx.update(|_, cx| theme::active_theme_name(cx)),
        "Pierre Light"
    );

    // A file that breaks while the app runs is toasted too; a reload that
    // finds the same errors does not toast them again.
    std::fs::write(dir.join("good.json"), "{ nope").unwrap();
    shell.cx.update(|_, cx| theme::reload(cx));
    shell.cx.update(|_, cx| theme::reload(cx));
    draw(shell.cx);
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    assert_eq!(
        toasts.iter().filter(|t| t.contains("good.json")).count(),
        1,
        "{toasts:?}"
    );
    assert_eq!(
        toasts.iter().filter(|t| t.contains("broken.json")).count(),
        1
    );
    assert!(
        shell
            .cx
            .update(|_, cx| ThemeRegistry::global(cx).get("Good").is_none())
    );

    // A theme name nobody has: Pierre instead, and one toast.
    shell.cx.update(|_, cx| {
        let mut s = Settings::default();
        s.theme.light = "Missing".into();
        SettingsStore::set(s, cx);
    });
    draw(shell.cx);
    assert_eq!(
        shell.cx.update(|_, cx| theme::active_theme_name(cx)),
        "Pierre Light"
    );
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    assert_eq!(
        toasts.iter().filter(|t| t.contains("\"Missing\"")).count(),
        1,
        "{toasts:?}"
    );
}

#[gpui_kit::test]
fn lilex_is_default_code_font(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    assert_eq!(Settings::default().buffer_font.family, LILEX_FAMILY);
    // The bundled faces are TrueType files of the Lilex family (the name
    // table spells it in UTF-16BE).
    let family_utf16: Vec<u8> = LILEX_FAMILY
        .encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect();
    for face in LILEX_FONTS {
        assert_eq!(&face[..4], &[0, 1, 0, 0], "a TrueType font");
        assert!(
            face.windows(family_utf16.len()).any(|w| w == family_utf16),
            "names the Lilex family"
        );
    }
    let mut shell = start(cx);
    // gpui-kit's monospace family is Lilex (registered before init_kit);
    // the UI keeps the system font.
    shell.cx.update(|_, cx| {
        let kit = Theme::global(cx);
        assert_eq!(kit.mono_font_family, LILEX_FAMILY);
        assert_eq!(kit.font_family, SYSTEM_UI_FONT);
    });
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let code_font = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).options().code_font.clone()
    });
    assert_eq!(code_font, LILEX_FAMILY);

    // Another code font in the settings: the viewport draws it, and
    // gpui-kit's monospace family follows the way init_kit picks it (never
    // Menlo by name, which would bring back its font scan: SF Mono).
    let set_family = |cx: &mut VisualTestContext, family: &str| {
        cx.update(|_, cx| {
            let mut s = Settings::default();
            s.buffer_font.family = family.into();
            SettingsStore::set(s, cx);
        });
        draw(cx);
    };
    let fonts = |cx: &mut VisualTestContext| {
        let code = tab.read_with(cx, |t, cx| t.viewport.read(cx).options().code_font.clone());
        let mono = cx.update(|_, cx| Theme::global(cx).mono_font_family.clone());
        (code.to_string(), mono.to_string())
    };
    set_family(shell.cx, "Menlo");
    assert_eq!(fonts(shell.cx), ("Menlo".into(), SYSTEM_MONO_FONT.into()));
    set_family(shell.cx, LILEX_FAMILY);
    assert_eq!(fonts(shell.cx), (LILEX_FAMILY.into(), LILEX_FAMILY.into()));
}
