//! Zed theme `style` colors → gpui-kit theme tokens (design §11.10,
//! ADR-0024: one palette drives the chrome, the syntax and the diff).
//!
//! gpui-kit reads its own theme schema (`ThemeConfig`, keys such as
//! `primary.background` or `tab.active.background`). [`kit_colors`] picks
//! each kit key's color from the Zed keys that mean the same thing (first
//! present wins, [`MAPPING`]), and derives the few Zed has no key for
//! (foregrounds on filled colors, from contrast). A key no Zed key covers is
//! left out, so gpui-kit derives it from the others the way it does for its
//! own theme files. The config never names a font: gpui-kit's families stay
//! the ones `polygloss_viewport::kit::init_kit` chose (naming `.SystemUIFont`
//! or `Menlo` would bring back its ≈ 440 ms font scan, plan T2.10.2).

use std::rc::Rc;

use gpui_kit::component::theme::{ThemeConfig, ThemeConfigColors, ThemeMode};
use polygloss_highlight::{Appearance, Rgba, ZedTheme};
use serde_json::{Map, Value};

/// A kit key and the Zed keys it takes its color from, in order.
pub const MAPPING: &[(&str, &[&str])] = &[
    ("background", &["editor.background", "background"]),
    ("foreground", &["text", "editor.foreground"]),
    ("border", &["border", "border.variant"]),
    ("input.border", &["border", "border.variant"]),
    ("ring", &["border.focused", "text.accent"]),
    (
        "muted.background",
        &["element.background", "surface.background"],
    ),
    ("muted.foreground", &["text.muted", "text.placeholder"]),
    (
        "accent.background",
        &["ghost_element.hover", "element.hover"],
    ),
    ("accent.foreground", &["text", "editor.foreground"]),
    (
        "primary.background",
        &["text.accent", "icon.accent", "border.focused"],
    ),
    (
        "secondary.background",
        &["element.background", "surface.background"],
    ),
    ("secondary.foreground", &["text", "editor.foreground"]),
    (
        "secondary.hover.background",
        &["element.hover", "ghost_element.hover"],
    ),
    (
        "secondary.active.background",
        &["element.active", "ghost_element.active"],
    ),
    (
        "popover.background",
        &["elevated_surface.background", "surface.background"],
    ),
    ("popover.foreground", &["text", "editor.foreground"]),
    (
        "list.hover.background",
        &["ghost_element.hover", "element.hover"],
    ),
    (
        "list.active.background",
        &["ghost_element.selected", "element.selected"],
    ),
    ("list.active.border", &["border.selected", "border.focused"]),
    (
        "sidebar.background",
        &["panel.background", "surface.background"],
    ),
    ("sidebar.foreground", &["text", "editor.foreground"]),
    ("sidebar.border", &["border", "border.variant"]),
    (
        "sidebar.accent.background",
        &["ghost_element.hover", "element.hover"],
    ),
    ("sidebar.accent.foreground", &["text", "editor.foreground"]),
    (
        "tab_bar.background",
        &["tab_bar.background", "title_bar.background"],
    ),
    (
        "tab.background",
        &["tab.inactive_background", "tab_bar.background"],
    ),
    (
        "tab.active.background",
        &["tab.active_background", "editor.background"],
    ),
    ("tab.foreground", &["text.muted", "text"]),
    ("tab.active.foreground", &["text", "editor.foreground"]),
    (
        "title_bar.background",
        &["title_bar.background", "background"],
    ),
    ("title_bar.border", &["border", "border.variant"]),
    (
        "status_bar.background",
        &["status_bar.background", "background"],
    ),
    ("status_bar.border", &["border", "border.variant"]),
    ("scrollbar.background", &["scrollbar.track.background"]),
    (
        "scrollbar.thumb.background",
        &["scrollbar.thumb.background"],
    ),
    (
        "scrollbar.thumb.hover.background",
        &["scrollbar.thumb.hover_background"],
    ),
    ("link", &["text.accent", "link_text.hover"]),
    ("link.hover", &["link_text.hover", "text.accent"]),
    ("drop_target.background", &["drop_target.background"]),
    ("drag.border", &["border.focused", "text.accent"]),
    (
        "danger.background",
        &["error", "deleted", "version_control.deleted"],
    ),
    (
        "success.background",
        &["success", "created", "version_control.added"],
    ),
    ("warning.background", &["warning", "modified"]),
    ("info.background", &["info", "text.accent"]),
    ("window.border", &["border", "border.variant"]),
    ("base.red", &["terminal.ansi.red", "error", "deleted"]),
    ("base.green", &["terminal.ansi.green", "success", "created"]),
    ("base.blue", &["terminal.ansi.blue", "info", "text.accent"]),
    ("base.yellow", &["terminal.ansi.yellow", "warning"]),
    ("base.cyan", &["terminal.ansi.cyan", "info"]),
    ("base.magenta", &["terminal.ansi.magenta"]),
];

/// The foregrounds on filled colors: `(kit key, the fill's kit key)`.
const CONTRAST: &[(&str, &str)] = &[
    ("primary.foreground", "primary.background"),
    ("danger.foreground", "danger.background"),
    ("success.foreground", "success.background"),
    ("warning.foreground", "warning.background"),
    ("info.foreground", "info.background"),
];

/// Every kit token the app's chrome paints with. A complete Zed theme (the
/// built-in Pierre themes) sets each of them; a partial user theme may not,
/// and gpui-kit derives the rest.
pub const REQUIRED_KIT_TOKENS: &[&str] = &[
    "background",
    "foreground",
    "border",
    "input.border",
    "ring",
    "muted.background",
    "muted.foreground",
    "accent.background",
    "accent.foreground",
    "primary.background",
    "primary.foreground",
    "secondary.background",
    "secondary.foreground",
    "popover.background",
    "popover.foreground",
    "list.hover.background",
    "list.active.background",
    "selection.background",
    "caret",
    "sidebar.background",
    "sidebar.foreground",
    "tab_bar.background",
    "tab.background",
    "tab.active.background",
    "tab.foreground",
    "tab.active.foreground",
    "title_bar.background",
    "title_bar.border",
    "scrollbar.thumb.background",
    "link",
    "danger.background",
    "success.background",
    "warning.background",
    "info.background",
    "base.red",
    "base.green",
    "base.blue",
    "base.yellow",
];

/// `#rrggbbaa`, as both Zed and gpui-kit write colors.
pub fn hex(c: Rgba) -> String {
    format!("#{:08x}", c.to_u32())
}

/// The kit color keys `t` sets, as `#rrggbbaa` strings.
pub fn kit_colors(t: &ZedTheme) -> Map<String, Value> {
    let mut colors = Map::new();
    let put = |colors: &mut Map<String, Value>, key: &str, c: Rgba| {
        colors.insert(key.to_owned(), Value::String(hex(c)));
    };
    for (kit, zed) in MAPPING {
        if let Some(c) = zed.iter().find_map(|k| t.color(k)) {
            put(&mut colors, kit, c);
        }
    }
    // The local player's selection and cursor (Zed's `players[0]`).
    let player = |key: &str| {
        t.style
            .get("players")
            .and_then(Value::as_array)
            .and_then(|p| p.first())
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .and_then(Rgba::parse)
    };
    let selection = player("selection")
        .or_else(|| t.color("editor.document_highlight.read_background"))
        .or_else(|| t.color("search.match_background"));
    if let Some(c) = selection {
        put(&mut colors, "selection.background", c);
    }
    if let Some(c) = player("cursor").or_else(|| t.color("text.accent")) {
        put(&mut colors, "caret", c);
    }
    for (fg, fill) in CONTRAST {
        let fill = colors
            .get(*fill)
            .and_then(Value::as_str)
            .and_then(Rgba::parse);
        if let Some(fill) = fill {
            put(&mut colors, fg, contrasting(fill));
        }
    }
    colors
}

/// White or near-black, whichever reads better on `fill` (WCAG relative
/// luminance; white up to about the luminance of a mid blue like Pierre's
/// accent).
pub fn contrasting(fill: Rgba) -> Rgba {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(fill.r) + 0.7152 * channel(fill.g) + 0.0722 * channel(fill.b);
    if luminance > 0.45 {
        Rgba {
            r: 0x0a,
            g: 0x0a,
            b: 0x0a,
            a: 0xff,
        }
    } else {
        Rgba {
            r: 0xff,
            g: 0xff,
            b: 0xff,
            a: 0xff,
        }
    }
}

/// gpui-kit's theme for `t`: its name, mode and [`kit_colors`], no fonts,
/// radius or syntax (the viewport paints the code).
pub fn kit_theme_config(t: &ZedTheme) -> Rc<ThemeConfig> {
    let colors: ThemeConfigColors = serde_json::from_value(Value::Object(kit_colors(t)))
        // Every value is a `#rrggbbaa` string under a key of the schema.
        .unwrap_or_default();
    Rc::new(ThemeConfig {
        name: t.name.clone().into(),
        mode: match t.appearance {
            Appearance::Light => ThemeMode::Light,
            Appearance::Dark => ThemeMode::Dark,
        },
        colors,
        ..ThemeConfig::default()
    })
}
