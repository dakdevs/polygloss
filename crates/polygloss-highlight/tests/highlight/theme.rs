use polygloss_highlight::{
    Appearance, FontStyle, PIERRE_DARK_JSON, PIERRE_LIGHT_JSON, Rgba, SyntaxStyle, ThemeError,
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
