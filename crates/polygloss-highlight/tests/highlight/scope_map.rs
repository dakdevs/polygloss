use polygloss_highlight::{Appearance, StyleId, SyntaxTheme, pierre_theme};

use crate::support::theme_with_keys;

#[test]
fn scope_longest_prefix_match() {
    let theme = SyntaxTheme::from_zed(&theme_with_keys(&[
        "keyword",
        "keyword.function",
        "punctuation.bracket",
        "string",
    ]));
    let keyword = theme.style_for_scope("keyword");
    let function = theme.style_for_scope("keyword.function");
    let bracket = theme.style_for_scope("punctuation.bracket");
    let ids = [keyword, function, bracket, theme.style_for_scope("string")];
    for (i, a) in ids.iter().enumerate() {
        assert_ne!(*a, StyleId::DEFAULT);
        for b in &ids[i + 1..] {
            assert_ne!(a, b, "each key has its own style");
        }
    }
    // Longest dotted prefix wins.
    assert_eq!(theme.style_for_scope("keyword.function.rust"), function);
    assert_eq!(theme.style_for_scope("keyword.function.go"), function);
    assert_eq!(theme.style_for_scope("keyword.return"), keyword);
    assert_eq!(theme.style_for_scope("keyword.import.typescript"), keyword);
    assert_eq!(
        theme.style_for_scope("punctuation.bracket.rainbow.1"),
        bracket
    );
    // Prefixes are whole dotted segments; no match is the default style.
    assert_eq!(
        theme.style_for_scope("punctuation.delimiter"),
        StyleId::DEFAULT
    );
    assert_eq!(theme.style_for_scope("punctuation"), StyleId::DEFAULT);
    assert_eq!(theme.style_for_scope("keywords"), StyleId::DEFAULT);
    assert_eq!(theme.style_for_scope("keyword_function"), StyleId::DEFAULT);
    assert_eq!(theme.style_for_scope(""), StyleId::DEFAULT);
    assert_eq!(theme.style_for_scope("variable"), StyleId::DEFAULT);
}

#[test]
fn style_returns_the_theme_entry() {
    let zed = theme_with_keys(&["comment", "keyword"]);
    let theme = SyntaxTheme::from_zed(&zed);
    assert_eq!(
        theme.style(theme.style_for_scope("keyword")),
        &zed.syntax["keyword"]
    );
    assert_eq!(
        theme.style(theme.style_for_scope("comment.documentation")),
        &zed.syntax["comment"]
    );
    assert_eq!(theme.style(StyleId::DEFAULT), &Default::default());
    // Unknown ids fall back to the default style instead of panicking.
    assert_eq!(theme.style(StyleId(u16::MAX)), &Default::default());
    assert_eq!(theme.styles().len(), 3);
}

#[test]
fn theme_id_is_stable_and_distinguishes_themes() {
    let light = SyntaxTheme::from_zed(pierre_theme(Appearance::Light));
    let dark = SyntaxTheme::from_zed(pierre_theme(Appearance::Dark));
    assert_eq!(
        light.id(),
        SyntaxTheme::from_zed(pierre_theme(Appearance::Light)).id()
    );
    assert_ne!(light.id(), dark.id());
    let a = SyntaxTheme::from_zed(&theme_with_keys(&["keyword"]));
    let b = SyntaxTheme::from_zed(&theme_with_keys(&["keyword", "string"]));
    assert_ne!(a.id(), b.id(), "same name, different syntax");
}
