//! Spacing tokens (T7.1, ADR-0031, design §11.17): the invariants each
//! group keeps, checked against independent sources (the 4 pt grid,
//! AppKit's control ladder from the macOS 26 probe, the reference's file
//! header and M6's geometry), never against restated token values.

use polygloss_viewport::space::{card, edge, gap, height, pad, radius, size, text};

/// Tokens that sit on the 2 pt half-step grid: values inside a component
/// (icon–label gaps, pill and field insets, the segmented rim, the sidebar
/// edge), and those derived from them.
const HALF_STEPS: &[&str] = &[
    "edge::SIDEBAR",
    "pad::ICON_LEAD",
    "pad::BADGE_X",
    "pad::RIM",
    "gap::ICON_LABEL",
    "size::ICON_SM",
    "size::DOT",
    "size::AVATAR_LG",
    "size::TREE_INDENT",
    "size::STATUS_COL",
    "card::CODE_PAD",
    "card::CODE_X",
];

/// `round(v, 1 decimal)`, so `55.6` compares with `55.600002`.
fn tenths(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

#[test]
fn every_layout_token_is_on_the_grid() {
    let groups: [(&str, &[(&str, f32)]); 6] = [
        ("edge", edge::ALL),
        ("pad", pad::ALL),
        ("gap", gap::ALL),
        ("height", height::ALL),
        ("size", size::ALL),
        ("card", card::ALL),
    ];
    let mut seen = Vec::new();
    for (group, tokens) in groups {
        assert!(!tokens.is_empty(), "space::{group}::ALL is empty");
        for (name, value) in tokens {
            let path = format!("{group}::{name}");
            let step = if HALF_STEPS.contains(&path.as_str()) {
                2.0
            } else {
                4.0
            };
            assert!(
                value >= &0.0 && value % step == 0.0,
                "space::{path} = {value} is not a multiple of {step}"
            );
            seen.push(path);
        }
    }
    for name in HALF_STEPS {
        assert!(
            seen.iter().any(|s| s == name),
            "the half-step list names {name}, which no group has"
        );
    }
}

#[test]
fn heights_are_appkits_ladder() {
    // mini, small, regular, large, extra large: NSControl.ControlSize's
    // fitting heights on macOS 26.6 (research: spacing audit, AppKit).
    let ladder = [16.0, 20.0, 24.0, 28.0, 36.0];
    assert_eq!(
        [
            height::MINI,
            height::XS,
            height::SM,
            height::MD,
            height::BAR
        ],
        ladder
    );
}

#[test]
fn radius_follows_role_then_height() {
    for (h, r) in [
        (16.0, 4.0),
        (20.0, 4.0),
        (22.0, 6.0),
        (24.0, 6.0),
        (28.0, 8.0),
        (36.0, 8.0),
        (44.0, 8.0),
        (52.0, 8.0),
    ] {
        assert_eq!(radius::for_height(h), r, "for_height({h})");
    }
    assert_eq!(radius::capsule(28.0), 14.0);
}

#[test]
fn gutter_grows_on_the_grid() {
    // Lilex at 13 pt: 7.8 pt per digit. One column: 4 (bar) + 4 + n · 7.8 +
    // 8, at least 40 and rounded up to the 4 pt grid.
    let advance = 7.8;
    for (digits, gutter) in [
        (1, 40.0),
        (2, 40.0),
        (3, 40.0), // 39.4
        (4, 48.0), // 47.2
        (5, 56.0), // 55
        (6, 64.0), // 62.8
    ] {
        assert_eq!(card::gutter(digits, advance, 1), gutter, "{digits} digits");
    }
    // Unified: two columns 8 apart, 4 + 4 + 2 · n · 7.8 + 8 + 8.
    assert_eq!(card::gutter(3, advance, 2), 72.0); // 70.8
    assert_eq!(card::gutter(4, advance, 2), 88.0); // 86.4
}

#[test]
fn code_x_follows_the_indicator_mode() {
    // Bars and none: 10 after the gutter. `+-`: the indicator's two-advance
    // cell instead, 40 + 2 · 7.8.
    assert_eq!(card::code_x(40.0, 7.8, false), 50.0);
    assert_eq!(tenths(card::code_x(40.0, 7.8, true)), 55.6);
}

#[test]
fn code_geometry_scales_with_the_row() {
    // Relational: code-holding geometry follows the code row.
    assert_eq!(card::header(24.0) - card::header(20.0), 4.0);
    assert_eq!(card::gap_row(24.0) - card::gap_row(20.0), 4.0);
    assert_eq!(card::placeholder(24.0) - card::placeholder(20.0), 8.0);
    // Anchored at the 20 pt row of 13 pt code: the reference's file header
    // is 46 pt from the card's top to the first row, less its two 1 pt
    // borders (ADR-0031 R10); M6's `Metrics::default` gap row and
    // placeholder were 32 and 48.
    assert_eq!(card::header(20.0), 46.0 - 2.0);
    assert_eq!(card::gap_row(20.0), 32.0);
    assert_eq!(card::placeholder(20.0), 48.0);
}

#[test]
fn text_line_heights_are_even_and_cover_the_size() {
    assert!(!text::ALL.is_empty());
    for (name, (size, line_height)) in text::ALL {
        assert_eq!(line_height % 2.0, 0.0, "text::{name}: {line_height} is odd");
        assert!(
            *line_height >= 1.2 * size,
            "text::{name}: {line_height} is under 1.2 × {size}"
        );
    }
}

#[test]
fn code_rows_are_one_and_a_half_sizes() {
    // The code-row rule kept from M6: 13 pt code → 20 pt rows (19.5 rounded
    // half up), 12 → 18, 16 → 24.
    assert_eq!(text::code_row(13.0), 20.0);
    assert_eq!(text::code_row(12.0), 18.0);
    assert_eq!(text::code_row(16.0), 24.0);
}
