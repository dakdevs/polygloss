//! Shared helpers: fixture paths and small themes.

use std::path::PathBuf;
use std::sync::Arc;

use polygloss_highlight::{SyntaxTheme, ZedTheme, load_theme_family};

/// `<repo>/fixtures/<rel>`.
pub fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

pub fn read_fixture(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

/// "Minimal Dark" from `fixtures/themes/minimal-zed-theme.json`: `keyword`,
/// `keyword.function`, `comment`, `string` and `punctuation` (invalid values).
pub fn minimal_theme() -> ZedTheme {
    let family = load_theme_family(&read_fixture("themes/minimal-zed-theme.json")).unwrap();
    family.themes.into_iter().next().unwrap()
}

pub fn minimal_syntax() -> Arc<SyntaxTheme> {
    Arc::new(SyntaxTheme::from_zed(&minimal_theme()))
}

/// A theme whose `syntax` map is exactly `keys`, each with a distinct color.
pub fn theme_with_keys(keys: &[&str]) -> ZedTheme {
    let syntax: serde_json::Map<String, serde_json::Value> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            (
                (*k).to_owned(),
                serde_json::json!({ "color": format!("#{:06x}ff", i + 1) }),
            )
        })
        .collect();
    let json = serde_json::json!({
        "name": "Keys",
        "themes": [{ "name": "Keys", "appearance": "light", "style": { "syntax": syntax } }]
    });
    load_theme_family(&json.to_string())
        .unwrap()
        .themes
        .remove(0)
}

/// `n` lines of plausible Rust, about 40 bytes each.
pub fn rust_source(n: usize) -> String {
    let mut s = String::with_capacity(n * 44);
    for i in 0..n {
        match i % 4 {
            0 => s.push_str(&format!("fn f{i}(x: u32) -> u32 {{\n")),
            1 => s.push_str(&format!("    let y = x + {i}; // note {i}\n")),
            2 => s.push_str("    y * 2\n"),
            _ => s.push_str("}\n"),
        }
    }
    s
}
