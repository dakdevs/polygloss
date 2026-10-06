//! Spacing tokens (ADR-0031, design §11.17): every layout dimension the
//! viewport and the app use, in absolute points (`f32`; call sites write
//! `px(space::height::SM)`, module-qualified, never glob-imported).
//!
//! The values sit on a 4 pt grid, with 2 pt half-steps only inside a
//! component; off the grid are only the 1 pt border and centring offsets,
//! which call sites compute (`(container − item) / 2`). Pick a token by the
//! relationship it describes: [`edge`] (a box's offset from its container's
//! edge), [`pad`] (a highlight's or field's edge to its content), [`gap`]
//! (between siblings), [`height`] (AppKit's control ladder), [`size`]
//! (objects), [`radius`] (by role, then by height), [`stroke`], [`text`]
//! (size and line height together) and [`card`] (the card's column grid).
//!
//! Each group exposes `ALL` (`(name, value)`) for the invariant tests and
//! the lint (`tests/scripts/spacing-tokens.test.ts`), which fails when a
//! `pub const` is missing from its group's `ALL`.

/// Where a box starts inside its container (the container applies it as
/// padding).
pub mod edge {
    /// Main column: the toolbar's first content box and last control, both
    /// outer edges of every card, the space below the last card, the banner
    /// strip's content; both top rows' trailing inset.
    pub const CANVAS: f32 = 12.0;
    /// Content inside a regular card (header card, Home card, dialog body),
    /// from its inner edge.
    pub const CARD_X: f32 = 16.0;
    pub const CARD_Y: f32 = 8.0;
    /// A card header's last trailing control, from the inner right edge.
    pub const CARD_TRAILING: f32 = 8.0;
    /// A card inside a card (thread blocks, the composer), from the outer
    /// card's inner edges; `NESTED_Y` also separates stacked nested cards.
    pub const NESTED_X: f32 = 12.0;
    pub const NESTED_Y: f32 = 8.0;
    /// Content inside a compact card: a nested card or a card in a dense
    /// panel.
    pub const COMPACT_X: f32 = 12.0;
    pub const COMPACT_Y: f32 = 8.0;
    /// Fields, row highlights, cards and plain text in every sidebar and the
    /// threads panel.
    pub const SIDEBAR: f32 = 10.0;
    /// Row highlights in palettes, finders and pickers.
    pub const OVERLAY: f32 = 8.0;
    /// Home's sides.
    pub const PAGE: f32 = 32.0;

    pub const ALL: &[(&str, f32)] = &[
        ("CANVAS", CANVAS),
        ("CARD_X", CARD_X),
        ("CARD_Y", CARD_Y),
        ("CARD_TRAILING", CARD_TRAILING),
        ("NESTED_X", NESTED_X),
        ("NESTED_Y", NESTED_Y),
        ("COMPACT_X", COMPACT_X),
        ("COMPACT_Y", COMPACT_Y),
        ("SIDEBAR", SIDEBAR),
        ("OVERLAY", OVERLAY),
        ("PAGE", PAGE),
    ];
}

/// From a highlight's, field's, pill's or badge's edge to its content.
pub mod pad {
    /// To the leading icon box.
    pub const ICON_LEAD: f32 = 6.0;
    /// To the text, when there is no icon.
    pub const TEXT: f32 = 8.0;
    /// Inside a pill, to its first box (icon or text).
    pub const PILL_X: f32 = 8.0;
    /// Inside a badge.
    pub const BADGE_X: f32 = 6.0;
    /// The segmented track's rim and segment gap; the filter field below the
    /// top row.
    pub const RIM: f32 = 2.0;

    pub const ALL: &[(&str, f32)] = &[
        ("ICON_LEAD", ICON_LEAD),
        ("TEXT", TEXT),
        ("PILL_X", PILL_X),
        ("BADGE_X", BADGE_X),
        ("RIM", RIM),
    ];
}

/// Between siblings.
pub mod gap {
    /// Title over subtitle: the line heights carry it.
    pub const LINES: f32 = 0.0;
    /// Glyph clusters (`+a −d`); a disclosure chevron to the icon after it.
    pub const INLINE: f32 = 4.0;
    /// Icon to label.
    pub const ICON_LABEL: f32 = 6.0;
    /// Sibling controls, and cards in a dense panel.
    pub const CONTROLS: f32 = 8.0;
    /// Groups and form fields.
    pub const GROUP: f32 = 12.0;
    /// Between canvas cards and between Home cards.
    pub const CARDS: f32 = 12.0;
    /// Sections and empty states.
    pub const SECTION: f32 = 24.0;

    pub const ALL: &[(&str, f32)] = &[
        ("LINES", LINES),
        ("INLINE", INLINE),
        ("ICON_LABEL", ICON_LABEL),
        ("CONTROLS", CONTROLS),
        ("GROUP", GROUP),
        ("CARDS", CARDS),
        ("SECTION", SECTION),
    ];
}

/// AppKit's control ladder (macOS 26: mini 16, small 20, regular 24, large
/// 28, extra large 36) and the bars built on it. A bar's height includes its
/// 1 pt divider; a card's border is outside its heights.
pub mod height {
    /// Badges, the gutter `+`.
    pub const MINI: f32 = 16.0;
    /// Inline buttons (`.xsmall()`).
    pub const XS: f32 = 20.0;
    /// Buttons (`.small()`), pills, segments, notices, leaf result lists.
    pub const SM: f32 = 24.0;
    /// Fields, segmented tracks, Viewed, list rows.
    pub const MD: f32 = 28.0;
    /// Pane header bars and section bands.
    pub const BAR: f32 = 36.0;
    /// The sidebar's footer.
    pub const FOOTER: f32 = 40.0;
    /// Two-line rows.
    pub const ROW2: f32 = 44.0;
    /// Both top rows.
    pub const TOP: f32 = 52.0;
    /// The banner strip above the first card (OQ-39): a notice and its
    /// margins.
    pub const BANNER_STRIP: f32 = SM + 2.0 * super::gap::INLINE;

    pub const ALL: &[(&str, f32)] = &[
        ("MINI", MINI),
        ("XS", XS),
        ("SM", SM),
        ("MD", MD),
        ("BAR", BAR),
        ("FOOTER", FOOTER),
        ("ROW2", ROW2),
        ("TOP", TOP),
        ("BANNER_STRIP", BANNER_STRIP),
    ];
}

/// Objects, not spacing.
pub mod size {
    /// Icons in toolbars, top rows, card headers and bands.
    pub const ICON: f32 = 16.0;
    /// Icons in sidebar rows and fields.
    pub const ICON_SM: f32 = 14.0;
    /// Disclosure chevrons in sidebars.
    pub const ICON_XS: f32 = 12.0;
    pub const DOT: f32 = 6.0;
    pub const AVATAR_SM: f32 = 20.0;
    /// The header card's avatar: sized so the title's box starts on the code
    /// column of a one-column card (`CARD_X + AVATAR_LG + CONTROLS` =
    /// `CODE_X`).
    pub const AVATAR_LG: f32 = super::card::CODE_X - super::edge::CARD_X - super::gap::CONTROLS;
    /// One tree depth.
    pub const TREE_INDENT: f32 = 14.0;
    /// The tree's status letter.
    pub const STATUS_COL: f32 = 10.0;

    pub const ALL: &[(&str, f32)] = &[
        ("ICON", ICON),
        ("ICON_SM", ICON_SM),
        ("ICON_XS", ICON_XS),
        ("DOT", DOT),
        ("AVATAR_SM", AVATAR_SM),
        ("AVATAR_LG", AVATAR_LG),
        ("TREE_INDENT", TREE_INDENT),
        ("STATUS_COL", STATUS_COL),
    ];
}

/// Corner radii, by role first: floating overlays (dialogs, popovers,
/// menus) → [`LG`]; cards → [`MD`]; pills, badges, chips, avatars and the
/// Viewed control → [`capsule`]. Everything else by height,
/// [`for_height`]. A concentric inner radius is [`inner`] when the inset is
/// 4 pt or less; with larger insets the role and height rules decide.
pub mod radius {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 6.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;

    /// The radius of a control `h` tall: up to 20 → XS, up to 24 → SM,
    /// taller → MD.
    pub const fn for_height(h: f32) -> f32 {
        if h <= 20.0 {
            XS
        } else if h <= 24.0 {
            SM
        } else {
            MD
        }
    }

    /// A pill's radius: half its height.
    pub const fn capsule(h: f32) -> f32 {
        h / 2.0
    }

    /// The concentric radius of a box `inset` inside one rounded at `outer`.
    pub const fn inner(outer: f32, inset: f32) -> f32 {
        outer - inset
    }

    pub const ALL: &[(&str, f32)] = &[("XS", XS), ("SM", SM), ("MD", MD), ("LG", LG)];
}

/// Line widths.
pub mod stroke {
    /// Borders and dividers.
    pub const BORDER: f32 = 1.0;
    pub const FOCUS_RING: f32 = 2.0;
    /// The cursor bar and selection rails: one width, color only.
    pub const CURSOR_BAR: f32 = 2.0;
    /// The change bar and Home's accent bar.
    pub const CHANGE_BAR: f32 = 4.0;

    pub const ALL: &[(&str, f32)] = &[
        ("BORDER", BORDER),
        ("FOCUS_RING", FOCUS_RING),
        ("CURSOR_BAR", CURSOR_BAR),
        ("CHANGE_BAR", CHANGE_BAR),
    ];
}

/// Text styles: a size and an even line height, set together (the app's
/// `TextStyleExt::text_style`).
pub mod text {
    /// `(size, line height)` in points.
    pub type Style = (f32, f32);

    /// Badges, 20 pt avatars, key caps.
    pub const CAPTION: Style = (11.0, 14.0);
    /// Secondary lines, pills, chips, counts, notices, the footer.
    pub const SMALL: Style = (12.0, 16.0);
    /// Comments and markdown, the header card's subtitle.
    pub const BODY: Style = (13.0, 18.0);
    /// Primary text of rows, fields, buttons and the toolbar's repo name.
    pub const UI: Style = (14.0, 20.0);
    /// Card and dialog titles.
    pub const TITLE: Style = (15.0, 20.0);
    /// Page headings and empty-state titles.
    pub const HEADING: Style = (16.0, 24.0);
    /// Numbers and hashes in chrome (`+a −d`, the SHA pill, totals), in the
    /// code font.
    pub const CODE_CHROME: Style = (12.0, 16.0);

    pub const ALL: &[(&str, Style)] = &[
        ("CAPTION", CAPTION),
        ("SMALL", SMALL),
        ("BODY", BODY),
        ("UI", UI),
        ("TITLE", TITLE),
        ("HEADING", HEADING),
        ("CODE_CHROME", CODE_CHROME),
    ];

    /// A code row's height at code size `size`: `round(1.5 × size)` (20 at
    /// 13 pt).
    pub fn code_row(size: f32) -> f32 {
        (1.5 * size).round()
    }
}

/// The card's column grid, from its inner left edge (ADR-0031 C1, C2):
/// the change bar, the number gutter, then code; the file header's chevron
/// box and path. Code-holding geometry scales with the code row; the rest
/// is fixed points.
pub mod card {
    use super::stroke::CHANGE_BAR;

    /// The file header's (and a section band's) chevron box.
    pub const CHEVRON_X: f32 = 12.0;
    /// The file path (and a band's label), in every card.
    pub const PATH_X: f32 = 40.0;
    /// The narrowest number gutter, change bar included.
    pub const GUTTER_MIN: f32 = 40.0;
    /// From the change bar to the number cells.
    pub const NUMBER_PAD_L: f32 = 4.0;
    /// From the last number cell to the gutter's end.
    pub const NUMBER_PAD_R: f32 = 8.0;
    /// Between unified's old and new number columns.
    pub const NUMBER_GAP: f32 = 8.0;
    /// From the gutter's end to the code (bars and none modes).
    pub const CODE_PAD: f32 = 10.0;
    /// Code in a one-column card with up to 3-digit numbers.
    pub const CODE_X: f32 = GUTTER_MIN + CODE_PAD;

    pub const ALL: &[(&str, f32)] = &[
        ("CHEVRON_X", CHEVRON_X),
        ("PATH_X", PATH_X),
        ("GUTTER_MIN", GUTTER_MIN),
        ("NUMBER_PAD_L", NUMBER_PAD_L),
        ("NUMBER_PAD_R", NUMBER_PAD_R),
        ("NUMBER_GAP", NUMBER_GAP),
        ("CODE_PAD", CODE_PAD),
        ("CODE_X", CODE_X),
    ];

    /// The number gutter's width, change bar included, for `columns` number
    /// columns (1 for a one-sided card and each split half, 2 for unified)
    /// of `digits` digits `advance` wide: at least [`GUTTER_MIN`], else the
    /// total rounded up to the 4 pt grid.
    pub fn gutter(digits: u32, advance: f32, columns: u32) -> f32 {
        let cells = columns as f32 * digits as f32 * advance;
        let gaps = columns.saturating_sub(1) as f32 * NUMBER_GAP;
        let total = CHANGE_BAR + NUMBER_PAD_L + cells + gaps + NUMBER_PAD_R;
        GUTTER_MIN.max(ceil_to_grid(total))
    }

    /// Where code starts: [`CODE_PAD`] after the gutter, or with `+-`
    /// indicators after their two-advance cell.
    pub fn code_x(gutter: f32, advance: f32, plus_minus: bool) -> f32 {
        if plus_minus {
            gutter + 2.0 * advance
        } else {
            gutter + CODE_PAD
        }
    }

    /// The file header's interior at code row `row`: between the card's top
    /// border and the separator above the first row (the header spans
    /// `header(row) + 2 · BORDER`).
    pub fn header(row: f32) -> f32 {
        row + 24.0
    }

    /// A gap (expander) row's height.
    pub fn gap_row(row: f32) -> f32 {
        row + 12.0
    }

    /// A placeholder's height (binary, submodule, generated, large files).
    pub fn placeholder(row: f32) -> f32 {
        2.0 * row + 8.0
    }

    /// `v` rounded up to the 4 pt grid. Totals within 0.001 pt above a grid
    /// line (float noise in `digits · advance`) stay on it.
    fn ceil_to_grid(v: f32) -> f32 {
        ((v - 0.001) / 4.0).ceil() * 4.0
    }
}
