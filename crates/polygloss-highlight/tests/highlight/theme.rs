use polygloss_highlight::{
    Appearance, FontStyle, PIERRE_DARK_JSON, PIERRE_LIGHT_JSON, POLYGLOSS_DARK_JSON,
    POLYGLOSS_LIGHT_JSON, Rgba, SyntaxStyle, ThemeError, ZedTheme, default_theme,
    load_theme_family, pierre_theme,
};

use crate::support::{fixture, read_fixture};

fn rgba(s: &str) -> Rgba {
    Rgba::parse(s).unwrap()
}

#[test]
fn load_pierre_light_and_dark() {
    for (json, appearance, file) in [
        (PIERRE_LIGHT_JSON, Appearance::Light, "pierre-light.json"),
        (PIERRE_DARK_JSON, Appearance::Dark, "pierre-dark.json"),
    ] {
        // The built-in constants are the committed port output.
        let on_disk = std::fs::read_to_string(fixture("../assets/themes").join(file)).unwrap();
        assert_eq!(json, on_disk, "{file}");

        let family = load_theme_family(json).unwrap();
        assert_eq!(family.themes.len(), 1, "{file}");
        let theme = &family.themes[0];
        assert_eq!(theme.appearance, appearance);
        assert_eq!(pierre_theme(appearance), theme);
        let expected_name = match appearance {
            Appearance::Light => "Pierre Light",
            Appearance::Dark => "Pierre Dark",
        };
        assert_eq!(theme.name, expected_name);
        assert_eq!(family.name, expected_name);

        // UI and diff colors the viewport reads.
        for key in [
            "background",
            "border",
            "text",
            "text.muted",
            "editor.background",
            "editor.foreground",
            "editor.line_number",
            "editor.active_line_number",
            "created",
            "created.background",
            "deleted",
            "deleted.background",
            "modified",
            "modified.background",
            "version_control.added",
            "version_control.deleted",
            "version_control.modified",
            "version_control.word_added",
            "version_control.word_deleted",
        ] {
            assert!(theme.color(key).is_some(), "{file}: style.{key}");
        }
        assert!(
            !theme.style.contains_key("syntax"),
            "syntax is lifted out of style"
        );
        // Word highlights are stronger than line backgrounds, same hue.
        for (line, word) in [
            ("created.background", "version_control.word_added"),
            ("deleted.background", "version_control.word_deleted"),
        ] {
            let (line, word) = (theme.color(line).unwrap(), theme.color(word).unwrap());
            assert_eq!((line.r, line.g, line.b), (word.r, word.g, word.b), "{file}");
            assert!(word.a > line.a, "{file}");
        }

        // Syntax captures, both Zed's names and the lumis (nvim-treesitter) ones.
        for key in [
            "attribute",
            "boolean",
            "comment",
            "constant",
            "function",
            "keyword",
            "number",
            "operator",
            "property",
            "punctuation",
            "string",
            "string.escape",
            "tag",
            "type",
            "variable",
            "variable.parameter",
            "markup.heading",
        ] {
            let style = theme.syntax.get(key);
            assert!(
                style.and_then(|s| s.color).is_some(),
                "{file}: syntax.{key}"
            );
        }
        assert_eq!(
            theme.syntax["markup.italic"].font_style,
            Some(FontStyle::Italic),
            "{file}"
        );
        assert_eq!(
            theme.syntax["markup.strong"].font_weight,
            Some(700),
            "{file}"
        );
    }

    // Spot-check the port against @pierre/theme 2.0.0's token colors.
    let light = pierre_theme(Appearance::Light);
    assert_eq!(light.syntax["keyword"].color, Some(rgba("#d32a61")));
    assert_eq!(light.syntax["string"].color, Some(rgba("#199f43")));
    assert_eq!(light.syntax["comment"].color, Some(rgba("#737373")));
    assert_eq!(light.color("editor.background"), Some(rgba("#ffffff")));
    let dark = pierre_theme(Appearance::Dark);
    assert_eq!(dark.syntax["keyword"].color, Some(rgba("#ff678d")));
    assert_eq!(dark.color("editor.background"), Some(rgba("#0a0a0a")));
}

/// The `polygloss.*` style keys (design §11.10, research "Polygloss palette").
const POLYGLOSS_KEYS: &[&str] = &[
    "polygloss.sidebar.field.background",
    "polygloss.created.line_number",
    "polygloss.created.gutter_background",
    "polygloss.deleted.line_number",
    "polygloss.deleted.gutter_background",
    "polygloss.stat.added",
    "polygloss.stat.deleted",
    "polygloss.commit_sha",
];

/// `players[i].cursor`.
fn player(t: &ZedTheme, i: usize) -> Option<Rgba> {
    t.style["players"][i]["cursor"]
        .as_str()
        .and_then(Rgba::parse)
}

#[test]
fn polygloss_themes_parse_with_pierres_key_set() {
    for (json, appearance, file, name) in [
        (
            POLYGLOSS_LIGHT_JSON,
            Appearance::Light,
            "polygloss-light.json",
            "Polygloss Light",
        ),
        (
            POLYGLOSS_DARK_JSON,
            Appearance::Dark,
            "polygloss-dark.json",
            "Polygloss Dark",
        ),
    ] {
        let on_disk = std::fs::read_to_string(fixture("../assets/themes").join(file)).unwrap();
        assert_eq!(json, on_disk, "{file}");
        let family = load_theme_family(json).unwrap();
        assert_eq!((family.name.as_str(), family.themes.len()), (name, 1));
        let theme = &family.themes[0];
        assert_eq!((theme.name.as_str(), theme.appearance), (name, appearance));

        // Every key Pierre sets, as a color, plus the `polygloss.*` keys.
        let pierre = pierre_theme(appearance);
        for key in pierre.style.keys().filter(|k| *k != "players") {
            assert!(theme.color(key).is_some(), "{file}: style.{key}");
        }
        for key in POLYGLOSS_KEYS {
            assert!(theme.color(key).is_some(), "{file}: style.{key}");
        }
        // Nothing else: a typo would be a key no reader looks up.
        for key in theme.style.keys() {
            assert!(
                key == "players"
                    || pierre.style.contains_key(key)
                    || POLYGLOSS_KEYS.contains(&key.as_str()),
                "{file}: unexpected style.{key}"
            );
        }
        // Pierre's syntax map, plus the captures the research adds.
        for key in pierre
            .syntax
            .keys()
            .map(String::as_str)
            .chain(["label", "function.macro"])
        {
            let style = theme.syntax.get(key);
            assert!(
                style.and_then(|s| s.color).is_some(),
                "{file}: syntax.{key}"
            );
        }
        assert_eq!(
            theme.syntax["markup.italic"].font_style,
            Some(FontStyle::Italic)
        );
        assert_eq!(theme.syntax["markup.strong"].font_weight, Some(700));
        // Eight players, each a cursor, background and selection color.
        let players = theme.style["players"].as_array().unwrap();
        assert_eq!(players.len(), 8, "{file}");
        for p in players {
            for key in ["cursor", "background", "selection"] {
                assert!(
                    p[key].as_str().and_then(Rgba::parse).is_some(),
                    "{file}: players {key}"
                );
            }
        }
    }

    // Spot-check against docs/research/redesign-reference.md "Polygloss
    // palette" (copied by hand).
    let light = load_theme_family(POLYGLOSS_LIGHT_JSON)
        .unwrap()
        .themes
        .remove(0);
    let dark = load_theme_family(POLYGLOSS_DARK_JSON)
        .unwrap()
        .themes
        .remove(0);
    for (key, l, d) in [
        ("background", "#f8f8f6", "#111113"),
        ("editor.background", "#ffffff", "#18181b"),
        ("surface.background", "#ffffff", "#18181b"),
        ("elevated_surface.background", "#ffffff", "#202023"),
        ("panel.background", "#ebebea", "#1c1c1f"),
        ("title_bar.background", "#fcfcfb", "#161618"),
        ("border", "#e7e7e7", "#2a2a2e"),
        ("element.background", "#f2f2f1", "#27272a"),
        ("ghost_element.hover", "#0000000a", "#ffffff0d"),
        ("text", "#18181b", "#ececed"),
        ("text.muted", "#6b6b73", "#a1a1aa"),
        ("editor.foreground", "#111111", "#e4e4e7"),
        ("editor.line_number", "#8e8e96", "#6b6b73"),
        ("text.accent", "#1f6feb", "#4c8dff"),
        ("created", "#57bb5c", "#4cc35a"),
        ("created.background", "#e6f6eb", "#1b2a1f"),
        ("deleted", "#e5605a", "#f0605a"),
        ("deleted.background", "#fdeded", "#2d1c1d"),
        ("version_control.word_added", "#bfe8c8", "#2c5434"),
        ("version_control.word_deleted", "#f7c9c6", "#5c2b2b"),
        ("version_control.added", "#3f9a45", "#5fcf6b"),
        ("version_control.modified", "#b7791f", "#e0a526"),
        ("version_control.deleted", "#c4433c", "#ff7b72"),
        ("version_control.renamed", "#7553c9", "#a48bf0"),
        ("polygloss.sidebar.field.background", "#dcdcdb", "#2a2a2e"),
        ("polygloss.created.line_number", "#3f9a45", "#5fcf6b"),
        ("polygloss.created.gutter_background", "#d9f2e0", "#1f3524"),
        ("polygloss.deleted.line_number", "#c4433c", "#ff8a80"),
        ("polygloss.deleted.gutter_background", "#fbe1df", "#3d2224"),
        ("polygloss.stat.added", "#3c7849", "#6bd17a"),
        ("polygloss.stat.deleted", "#aa3c36", "#ff7b72"),
        ("polygloss.commit_sha", "#a8621f", "#e3a25b"),
    ] {
        assert_eq!(light.color(key), Some(rgba(l)), "light {key}");
        assert_eq!(dark.color(key), Some(rgba(d)), "dark {key}");
    }
    for (key, l, d) in [
        ("keyword", "#342dda", "#a39dff"),
        ("string", "#3a8a3f", "#7ccf7f"),
        ("string.escape", "#2a8a9e", "#5cc1d6"),
        ("type.builtin", "#a8621f", "#e3a25b"),
        ("comment", "#86868b", "#85858c"),
        ("function", "#2e407d", "#9fb4f0"),
        ("variable", "#111111", "#e4e4e7"),
        ("tag.delimiter", "#6b6b73", "#a1a1aa"),
    ] {
        assert_eq!(light.syntax[key].color, Some(rgba(l)), "light {key}");
        assert_eq!(dark.syntax[key].color, Some(rgba(d)), "dark {key}");
    }
    assert_eq!(player(&light, 0), Some(rgba("#1f6feb")));
    assert_eq!(player(&light, 7), Some(rgba("#6b7280")));
    assert_eq!(player(&dark, 0), Some(rgba("#4c8dff")));
    assert_eq!(player(&dark, 7), Some(rgba("#9ca3af")));
}

#[test]
fn default_theme_is_polygloss_per_appearance() {
    for (appearance, json, name) in [
        (Appearance::Light, POLYGLOSS_LIGHT_JSON, "Polygloss Light"),
        (Appearance::Dark, POLYGLOSS_DARK_JSON, "Polygloss Dark"),
    ] {
        let theme = default_theme(appearance);
        assert_eq!(theme.name, name);
        assert_eq!(theme.appearance, appearance);
        assert_eq!(*theme, load_theme_family(json).unwrap().themes[0]);
    }
    // Pierre stays available by its own constructor.
    assert_eq!(pierre_theme(Appearance::Light).name, "Pierre Light");
    assert_eq!(pierre_theme(Appearance::Dark).name, "Pierre Dark");
}

#[test]
fn load_minimal_zed_theme_ignores_unknown_keys() {
    let family = load_theme_family(&read_fixture("themes/minimal-zed-theme.json")).unwrap();
    assert_eq!(family.name, "Minimal");
    assert_eq!(family.author.as_deref(), Some("Polygloss tests"));
    assert_eq!(family.themes.len(), 2);

    let dark = &family.themes[0];
    assert_eq!(dark.name, "Minimal Dark");
    assert_eq!(dark.appearance, Appearance::Dark);
    // UI colors: every Zed color form; null, invalid and non-color values are None.
    assert_eq!(dark.color("background"), Some(rgba("#1e1e1eff")));
    assert_eq!(dark.color("editor.background"), Some(rgba("#1e1e1e")));
    assert_eq!(
        dark.color("deleted"),
        Some(Rgba {
            r: 0xff,
            g: 0,
            b: 0,
            a: 0xff
        })
    );
    assert_eq!(dark.color("modified"), None);
    assert_eq!(dark.color("border.variant"), None);
    assert_eq!(dark.color("players"), None);
    assert_eq!(dark.color("missing"), None);
    assert_eq!(dark.color("some.future.color"), Some(rgba("#12345678")));

    // Syntax: known fields parsed, unknown fields and invalid values ignored,
    // non-object entries skipped.
    assert_eq!(
        dark.syntax.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "comment",
            "keyword",
            "keyword.function",
            "punctuation",
            "string"
        ]
    );
    assert_eq!(
        dark.syntax["keyword"],
        SyntaxStyle {
            color: Some(rgba("#ff0000ff")),
            ..SyntaxStyle::default()
        }
    );
    assert_eq!(dark.syntax["keyword.function"].font_weight, Some(700));
    assert_eq!(dark.syntax["comment"].font_style, Some(FontStyle::Italic));
    assert_eq!(dark.syntax["comment"].color, Some(rgba("#80808080")));
    assert_eq!(
        dark.syntax["string"].background_color,
        Some(rgba("#00000000"))
    );
    assert_eq!(dark.syntax["punctuation"], SyntaxStyle::default());

    // A theme without `style` is valid and empty.
    let light = &family.themes[1];
    assert_eq!(light.appearance, Appearance::Light);
    assert!(light.style.is_empty());
    assert!(light.syntax.is_empty());
}

#[test]
fn load_theme_family_rejects_malformed_documents() {
    assert!(matches!(
        load_theme_family("not json"),
        Err(ThemeError::Json(_))
    ));
    assert!(matches!(load_theme_family("[]"), Err(ThemeError::Json(_))));
    assert!(matches!(
        load_theme_family(r#"{"name":"Empty","themes":[]}"#),
        Err(ThemeError::NoThemes(name)) if name == "Empty"
    ));
    // Name and appearance are required; appearance is light or dark.
    assert!(matches!(
        load_theme_family(r#"{"name":"X","themes":[{"name":"X"}]}"#),
        Err(ThemeError::Json(_))
    ));
    assert!(matches!(
        load_theme_family(r#"{"name":"X","themes":[{"name":"X","appearance":"sepia"}]}"#),
        Err(ThemeError::Json(_))
    ));
    assert!(matches!(
        load_theme_family(r#"{"themes":[{"name":"X","appearance":"dark"}]}"#),
        Err(ThemeError::Json(_))
    ));
}

#[test]
fn rgba_parses_zed_color_forms() {
    let c = |r, g, b, a| Some(Rgba { r, g, b, a });
    assert_eq!(Rgba::parse("#abc"), c(0xaa, 0xbb, 0xcc, 0xff));
    assert_eq!(Rgba::parse("#abcd"), c(0xaa, 0xbb, 0xcc, 0xdd));
    assert_eq!(Rgba::parse("#a1b2c3"), c(0xa1, 0xb2, 0xc3, 0xff));
    assert_eq!(Rgba::parse("#A1B2C3D4"), c(0xa1, 0xb2, 0xc3, 0xd4));
    for bad in [
        "",
        "#",
        "abc",
        "#12345",
        "#1234567",
        "#gggggg",
        "#123456789",
        " #123456",
        "#ab€",
    ] {
        assert_eq!(Rgba::parse(bad), None, "{bad:?}");
    }
    assert_eq!(
        Rgba {
            r: 1,
            g: 2,
            b: 3,
            a: 4
        }
        .to_u32(),
        0x0102_0304
    );
}
