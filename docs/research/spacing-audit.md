# Spacing audit (M7)

Findings behind [ADR-0031](../adr/0031-spacing-system.md) and plan M7, gathered 2026-10-06 on `polygloss-v1` at `d5e36ca` (M6 through T6.15). T6.16, the visual parity pass, edits these same literals: the M7 spacing tasks re-measure on the merged tree instead of trusting the line numbers or counts here.

## Method

- **Reference:** the redesign reference ([research](redesign-reference.md); not in the repo), decoded at 2 px per pt with a pure-Python PNG reader; window left edge at x = 63 px, top at y = 70 px; edges read from pixel runs. Origins are the reference's own 1 pt edges: the sidebar's divider at px 623–624 (`#cbd3d4` beside the sidebar), so the main column starts at px 625 (281 pt), and each card's gray border (`#e7e7e7` on the `#f8f8f6` canvas; the file card's at px 649–650, 293 pt). Ink is a pixel more than 24 off its background in at least one channel. Accuracy about ± 0.5 pt. Round 1 started card-relative edges at the change bar (px 651), 1 pt inside the border, which a 24-per-channel scan does not see; every card-relative number below is re-measured from px 649.
- **Polygloss:** the same decode of `e2e-shell-review-tab.png`, `e2e-tree-accordion.png` and `e2e-threads-split.png` at `d5e36ca`, or T6.16's measurements where it re-measured (OQ-56). M7's tests re-measure on the merged tree, with origins from layout bounds.
- **AppKit:** a Swift probe on macOS 26.6.2 instantiated each control at each `controlSize` and read `fittingSize`, table and outline metrics, the unified toolbar's traffic-light frames and the system font's line heights.
- **Code:** every spacing-relevant call in `polygloss-app/src` and `polygloss-viewport/src` outside `#[cfg(test)]`, by a scanner with the rules T7.1's lint uses.

## Reference vs Polygloss

| Measurement (pt)                                            | Polygloss                                                                                | Reference                                                                   |
| ----------------------------------------------------------- | ---------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Sidebar, divider, main column                               | 280; 1, drawn by gpui-kit's split handle over the main column's first point (T6.16); 280 | 280, 1, 281                                                                 |
| Canvas side margin (card outer edge to the main column)     | 16                                                                                       | 12 left (from px 625, after the divider), 12 right                          |
| Toolbar bottom (its 1 pt rule included) to the first card   | 32 (strip)                                                                               | 11                                                                          |
| Gap between cards                                           | 12                                                                                       | 12                                                                          |
| Header card height (borders included)                       | 60                                                                                       | 56.5                                                                        |
| Header card: avatar x / title ink x (card's outer edge)     | 16.5 / —                                                                                 | 17 / 53.5 (`init`)                                                          |
| File header (plus border)                                   | 43 + 1                                                                                   | 44 + 1                                                                      |
| Change bar                                                  | 3                                                                                        | 4 (px 651–658, inside the border)                                           |
| File header chevron ink, from the card's outer edge         | 8.5 (T6.16)                                                                              | 15.5 (a 16 pt chevron-down: 9 × 5 pt of ink)                                |
| File path ink                                               | 24.5 (T6.16)                                                                             | 42 (`i`; 42.5 at full strength)                                             |
| Numbers' right ink edge / gutter tint's end / code ink      | — / ≈ 32 / ≈ 36                                                                          | 34 / 42.5 / 53 (`//`)                                                       |
| Toolbar first glyph ink, from the main column's edge        | 11.5 (cards at 16)                                                                       | 14.5 (`godiff`; cards at 12)                                                |
| Toolbar's trailing control vs the card's right edge         | not aligned                                                                              | equal (x = 2363 px)                                                         |
| Sidebar outer inset (filter field / footer text)            | 8 / 12.5                                                                                 | 10 / 10.5                                                                   |
| Tree, depth 0: chevron / folder / label ink                 | 15.5 / — / —                                                                             | 17.5 / 33.5 / 52.5 (a 12 pt chevron, 7 × 4; a 14 pt folder, 13 × 11.5)      |
| Tree, depth 1 and a depth-2 file                            | —                                                                                        | chevron 31.5, folder 47.5, label 66.5; `file` icon 62.5 (14 pt), label 80.5 |
| Tree stats' right edge to the sidebar's right edge          | 38                                                                                       | 19                                                                          |
| Sidebar toggle ink's right edge to the sidebar's right edge | 15.5                                                                                     | 17                                                                          |
| Filter field: height / top                                  | 28 / 56                                                                                  | 28 / 54                                                                     |
| Tree row pitch / code row                                   | 28 / 20                                                                                  | ≈ 29 / 20                                                                   |
| Footer (plus border)                                        | 38                                                                                       | 39 + 1                                                                      |
| Pills (toolbar ±, file-header ±, branch)                    | 24 capsules (toolbar); 22 at radius 6 (header)                                           | 22 capsules                                                                 |
| Viewed pill                                                 | 28 at radius 6                                                                           | 30 capsule, bordered                                                        |
| Segmented tracks                                            | 28 (sidebar), 26 (layout)                                                                | 28 both                                                                     |
| Close button ink / filter icon ink                          | 19 / ≈ 16                                                                                | 19 / 19                                                                     |

## AppKit (macOS 26.6.2 probe)

| Item                                          | Value                                                                                                               |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| Push button, segmented control, popup by size | mini 16, small 20, regular 24, large 28, extra large 36 (macOS 26 made mini to medium taller and added extra large) |
| Text field, search field                      | 24                                                                                                                  |
| `NSTableView` row height / intercell          | 24 / (17, 0)                                                                                                        |
| Source-list outline indent                    | 12                                                                                                                  |
| Unified-toolbar traffic lights                | 14 pt each at x = 19, 42, 65, top 19; the zoom button ends at 79                                                    |
| System font line height                       | 12 pt → 15, 13 → 16, 14 → 17, 15 → 18                                                                               |

The HIG gives no macOS point values for layout, toolbars or sidebars. It asks for alignment, indentation for hierarchy, consistent spacing and corner radii concentric with their container; its 44 pt hit region is cross-platform and its 28/32/44 pt button shapes are visionOS.

## Counts

Scanner rules (T7.1's lint): 493 matches in 47 files.

| Rule                                                           | Matches |
| -------------------------------------------------------------- | ------- |
| Raw `px(<number>)` (not `px(0.)`)                              | 110     |
| gpui rem spacing and size helpers (`p_2`, `gap_1p5`, `h_8`, …) | 222     |
| Text presets (`text_xs`, `text_sm`, …)                         | 100     |
| Literal line heights                                           | 8       |
| Numeric `f32` / `Pixels` geometry consts                       | 53      |

Largest files (the audit's own count of spacing calls; the lint's per-file numbers differ slightly): `open_flow/source_step.rs` 42, `threads/block.rs` 37, `home/row.rs` 35, `threads/panel.rs` 30, `review_tab/header.rs` 30, `find/view.rs` 30, `composer/view.rs` 29, `chrome.rs` 28, `submit/dialog.rs` 25, `open_flow/mod.rs` 23, `composer/mod.rs` 22, `tree/row.rs` 19, `review_tab/banners.rs` 16; viewport `paint_rows.rs` 13, `card.rs` 13.

| Kind           | Values in use (count)                                                                                                |
| -------------- | -------------------------------------------------------------------------------------------------------------------- |
| Padding        | 0 ×4, 1 ×1, 2 ×10, 3 ×1, 4 ×26, 6 ×16, 8 ×34, 10 ×7, 12 ×20, 16 ×8, 24 ×7, 32 ×1, 40 ×1                              |
| Gap            | 2 ×11, 4 ×25, 6 ×8, 8 ×28, 10 ×1, 12 ×16, 16 ×2, 20 ×1, 32 ×1                                                        |
| Literal radius | 5 ×3, 6 ×12, 7 ×2, 8 ×2, 12 ×1; `theme.radius` (6) ×8, `radius_lg` (10) ×3                                           |
| Fixed heights  | 16, 20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 44, 45, 46, 48, 52, 60, 64                                           |
| Secondary bars | 30 (accordion header), 32 (find header, banner strip), 34 (composer bar), 38 (footer), 40 (threads header)           |
| Radii          | 3, 4, 5, 6, 7, 8, 10, 12 and capsules                                                                                |
| Text sizes     | 10, 12 (`text_xs` ×58), 13 (×4, plus every viewport label), 14 (`text_sm` ×40), 15 (×2), 16 (×2)                     |
| Line heights   | gpui's default 1.618 (19.4 at 12 pt, 22.65 at 14); code 1.5 ×; overrides 16, 17, 18, relative 1.2 and 1.54 ×         |
| Units          | rem helpers ≈ 330, `px` literals 93, code-font advances in the viewport (3 a, 2 a, 0.5 a; a ≈ 7.8 pt at 13 pt Lilex) |

Not in the counts: struct-literal geometry and painter arithmetic in the viewport (`CardStyle::default` `margin_x: 16.0, gap: 12.0, radius: 8.0, pad_bottom: 8.0` at `card.rs:42-46`; `Metrics::default` `header_height: 40.0, band_height: 36.0, gap_height: 32.0, placeholder_height: 48.0` at `document/metrics.rs:48-54`; `.clamp(0.0, 4.0)` at `paint_rows.rs:500`, `style.radius + 2.0` at `card.rs:173`, `0.5 * a` at `section_band.rs:413` and `gap.rs:358`), `gpui_kit::rems(1.2)` (`markdown/sanitize.rs:420`) and computed line heights (`relative(1.2)` at `review_tab/toolbar.rs:347,355`, `relative(1.)` at `palette/key_cap.rs:42`, `px((size * 1.54).round())` at `markdown/suggestion.rs:259` and `threads/block.rs:365`). About 160 float literals sit in viewport `src` outside tests, geometry and color math mixed; T7.1's rule 6, on every viewport file, separates them (blocks.rs and selection.rs paint through `Painter::{quad, rounded, icon, text}`, not `paint_quad`).

## Icon ink at 16 pt

Lucide draws on a 24-unit grid with a 2-unit stroke (`gpui-kit-assets` 0.7.0), so the ink starts inside the box by the glyph's own inset, stroke included (at 16 pt × 2/3, at 14 pt × 7/12, at 12 pt × 1/2 of the 24-unit inset):

| Glyph                                | Path's leftmost extent (24 units) | Ink inset at 16 pt |
| ------------------------------------ | --------------------------------- | ------------------ |
| `folder`                             | x 2, stroke to 1                  | 0.7                |
| `search`                             | circle at 11, r 8, stroke to 2    | 1.3                |
| `panel-left`, `panel-left-open`      | rect at 3, stroke to 2            | 1.3 (both sides)   |
| `chevron-down` (open)                | `m6 9`, stroke to 5               | 3.3                |
| `chevron-right` (closed)             | `m9 18`, stroke to 8              | 5.3                |
| At 14 pt: `folder`, `search`, `file` | x 1, 2, 3 with the stroke         | 0.6, 1.2, 1.8      |
| At 12 pt: `chevron-down`             | stroke to 5                       | 2.5                |

ADR-0031 therefore defines columns by box and records each glyph's ink in its reference-edges table.

## gpui-kit

- gpui's rem helpers resolve against gpui-component's `Theme.font_size`, 16 by default (`root.rs:436`); Polygloss never sets it, so `p_2` = 8. At a native 13 every helper would scale by 0.8125 to fractional points.
- `gpui_base::SpacingTokens` 2/4/8/12/16/24/32 and `RadiusTokens` 0/3/6/8/12/full (`theme_tokens.rs:109-153`): unused, and the kit's components do not read them.
- `zed_to_kit.rs:285-286` sets the kit's `radius` 6 and `radius_lg` 10: kit dialogs and popovers are 10 while our cards are 8.
- Kit control heights: Button xsmall 20, small 24, medium 32; small Input 24; `ListItem` `px_3 py_1`; menus snap with an 8 pt margin.

## Bugs found

1. Four left edges in one sidebar: filter field 8, tree content 12, Reviews icons 16, footer 12; panel-header chevron 8 vs tree chevron 12.
2. The two top rows' trailing insets differ (8 vs 12), and the toolbar sits 4.5 pt off the card edge.
3. Two near-duplicate segmented controls (30 × 24 and 28 × 22 segments, radius 5 in a radius-7 track).
4. File-header pills at radius 6 where every other pill is a capsule; three pill heights for one role (22, 24, 28).
5. Header and band columns in code-font advances: fractional, and they move with the code font; number-to-code ≈ 4 pt against the reference's 8 + 10.
6. Thread blocks at 10 / 6 margins and radius 6 inside radius-8 cards; the composer repeats 10 / 6 as literals (`composer/mod.rs:797-799`); two gutter formulas (`size · 0.62 · (digits + 1) + 10`, and `+ 12` for suggestions).
7. Off-scale values: `py_2p5` on the header card, `gap_2p5`/`px_2p5`/`pt(1)`/size 7 in the Submit dialog, 34 pt composer bar, 22 pt layout segments, `pb_10` on Home.
8. Pickers share a 560 pt width but use three max heights (380, 400, 420) and five row heights for one kind of list (30, 44, 46, 48, 26).
9. Home: two pill shapes on one row (radius-6 kind badge, capsule status); cards 8 apart while canvas cards are 12.
10. Composer editor double padding (`px_3 py_2` around `px_1 py_1`); reply rows at 12/8 and 8/8.
11. The tree's always-reserved 20 pt Viewed slot puts the stats 38 pt from the edge (OQ-43).
12. Live counters in the proportional UI font without tabular figures (`rg 'tnum|tabular'` finds nothing).

## Value → token map

Tokens are ADR-0031's (`edge`, `pad`, `gap`, `height`, `size`, `radius`, `stroke`, `text`, `card`). "Bug" marks a value that changes meaning, not only its name.

### Window chrome and toolbar

| Today                                                          | Token                                                                                                                                        |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Sidebar top row `pl` 80 (12 in fullscreen), `pr_2` 8           | `TOOLBAR_INSET_HIDDEN` 88 (derived) / `CANVAS`; `pr` `CANVAS` (both top rows' trailing inset), so the toggle's icon box ends at W − 16 (bug) |
| Toolbar `px_3`, `gap_2`; inset `pl` 80                         | `CANVAS`; `CONTROLS`; 88                                                                                                                     |
| Segmented controls 30 × 24 and 28 × 22, rim and gap 2, r5 / r7 | One `SegmentedControl`: track `MD` 28, rim and gap `RIM`, segments `SM` 24 × 28, radii `MD` / `SM` (bug)                                     |
| Ghost sidebar toggle 24 at r6; toolbar pills 24 `rounded(12)`  | `SM` / radius `SM`; `capsule(SM)` (unchanged)                                                                                                |
| Repo block `text_sm` / `text_xs` at line height 1.2            | `text::UI` over `text::SMALL` (36 in 52)                                                                                                     |

### Sidebar, tree, footer, find, Reviews list

| Today                                                                                                                 | Token                                                                                                                                                                                                 |
| --------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Filter wrapper `px_2 pt_1 pb_2`; field `min_h` 28 r8                                                                  | x `SIDEBAR` 10, top 54 (`TOP` + `RIM`), `pb` `CONTROLS`; `MD` at radius `MD`; icon box at `ICON_LEAD`                                                                                                 |
| Accordion header 30, `px_2`, `gap_1p5`                                                                                | `MD` 28; `ICON_XS` chevron box at 16 (S2 rule), label `ICON_LABEL` after it (34) (bug: chevrons at 8 vs 12)                                                                                           |
| Tree body `px_1p5` + row `pl` 6; chevron slot `w(14)` with an `.xsmall()` (12) chevron; 16 pt `folder` / `file` icons | Highlight `SIDEBAR`; chevron `ICON_XS` box at 16 (`ICON_LEAD`), `INLINE`, icon `ICON_SM` box at 32, `ICON_LABEL`, label at 52; files keep the chevron's slot (bug: 16 pt icons vs the reference's 14) |
| Tree row 28, INDENT 14, `SLOT` 20, `pr_1` 4, literal `rounded(px(6.))`                                                | `MD`; `TREE_INDENT`; `layout::VIEWED_SLOT`; `TEXT`; radius `MD` by height (bug: literal)                                                                                                              |
| Tree inner gaps 6/6/6/4; dot 6; thread pill `px_1p5`; status column 10                                                | `ICON_LABEL`, `INLINE`; `DOT`; `BADGE_X` capsule; `STATUS_COL`                                                                                                                                        |
| Kit `ListItem` `px_3 py_1 gap_x_1`                                                                                    | Overridden with tokens                                                                                                                                                                                |
| Footer 38, `px_3`, `gap_1`, `w(6)` spacer; chips `px(6)` capsules                                                     | `FOOTER` 40 incl. border; `SIDEBAR` for its text; `INLINE`; `GROUP` gap (bug: spacer); `BADGE_X`                                                                                                      |
| Reviews list `px_2`, `gap_0p5`; rows 28 `px_2 gap_2` r6; section rows `mt_2 pl_2 pr_1`                                | `SIDEBAR`; 0 (rows touch); `MD`, `ICON_LEAD`, `ICON_LABEL`, radius `MD`; `CONTROLS`, `TEXT`                                                                                                           |
| Find header 32 `pl_3 pr_1 gap_1 ml_1`; input row `px_2 py_1p5`                                                        | `BAR` 36; S2 rule, `INLINE`; the filter field's geometry                                                                                                                                              |
| Find results 24 `pl_3` / `pl_2` `gap_1p5`; number column 40                                                           | `SM`; one left edge (bug: two); `ICON_LABEL`; `FIND_NUMBER_COL`                                                                                                                                       |
| Match chips `px_1p5` capsule; context chips `px_1` r6                                                                 | `BADGE_X` capsule for both (bug)                                                                                                                                                                      |

### Canvas, header card, banners

| Today                                                                      | Token                                                                                                                      |
| -------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `CardStyle` margin 16, gap 12, radius 8, bottom pad 8, border 1            | `CANVAS` 12 (sides and below the last card; bug vs the reference), `CARDS`, `MD`, `CARD_Y`, `BORDER`                       |
| Header card `px_4`, `py_2p5`, `gap_3`, right `gap_4`, literal `rounded(8)` | `CARD_X`; `CARD_Y` (bug: off-scale); `CONTROLS` (title on the code column); `GROUP`; `CardStyle`'s radius (bug: duplicate) |
| Avatar 26; title 15/18; subtitle 13/17                                     | `AVATAR_LG` (derived); `text::TITLE` 15/20; `text::BODY` 13/18                                                             |
| Commit rows 28 `px_4 gap_2 py_1`, avatar 18 at text 10                     | `MD`, `CARD_X`, `CONTROLS`, `INLINE`; `AVATAR_SM` 20, `text::CAPTION` 11                                                   |
| Banner strip 32 `px_4`                                                     | `BANNER_STRIP` 32 (OQ-39); `CANVAS`                                                                                        |
| Notices 24 `pl_2 gap_2` r6, dot 6, border 1, travel 4                      | `SM`, `TEXT`, `CONTROLS`, radius `SM`, `DOT`, `BORDER`, motion's `NUDGE` 4 (`ENTER_FROM_ABOVE` leaves with T7.9)           |

### File header, rows, gutter, gap rows, bands

| Today                                                                  | Token                                                                                                                                                                    |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `GAP` 6, `ICON` 16, `SMALL_ICON` 14, `ICON_GAP` 4, `BUTTON` 24         | `ICON_LABEL`, `ICON`, `ICON_SM`, `INLINE`, `SM`                                                                                                                          |
| `PILL_PAD` 6, `PILL_RADIUS` 6; pills row + 2 (22)                      | `PILL_X` 8; `capsule(SM)` at `SM` 24 (bug vs the reference's capsules)                                                                                                   |
| `VIEWED_PAD` 8, `VIEWED_GAP` 6; Viewed row + 8 (28); open-in-editor 24 | `TEXT`, `ICON_LABEL`; `MD` (fixed) at `capsule(MD)`; `SM`                                                                                                                |
| Inline +4 icon inset                                                   | `(SM − ICON) / 2`, derived                                                                                                                                               |
| Chevron and ⋯ columns 3 advances                                       | `CHEVRON_X` 12; path at `PATH_X` 40; ⋯ ends `CARD_TRAILING` from the inner edge (bug: fractional, scales with the font)                                                  |
| Header `round(2.25 · row)` = 45; `Metrics::default` header 40          | `card::header(row)` = row + 24 (44 at 13 pt), plus `BORDER`                                                                                                              |
| Rows `round(1.5 · size)`                                               | Unchanged                                                                                                                                                                |
| Gap row 1.6 · row; placeholder 2.4 · row                               | `card::gap_row` = row + 12; `card::placeholder` = 2 · row + 8 (equal at 13 pt)                                                                                           |
| Number column `(digits + 1)` advances; indicator 0.5 advance           | `card::gutter(digits, advance, columns)`: one gutter over unified's two columns `NUMBER_GAP` apart; then `CODE_PAD` 10, or 2 advances in `+-` mode (bug: ≈ 4 pt to code) |
| `BAR_WIDTH` 3, `CURSOR_BAR_WIDTH` 2, `HOVER_RADIUS` 6                  | `CHANGE_BAR` 4 (bug vs the reference's 4), `CURSOR_BAR`, `radius::for_height` of the hovered control                                                                     |
| Gutter `+` button row − 4 at r4                                        | `MINI` 16 at radius `XS` (fixed UI geometry)                                                                                                                             |
| Band 36, `BAND_PAD` 4, `DOT` 6, chevron 3 advances                     | `BAR` 36; `INLINE`; `DOT`; `CHEVRON_X` 12, label at `PATH_X`                                                                                                             |
| Gap-row links 2 and 0.5 advances apart                                 | From the code column, `GROUP` apart                                                                                                                                      |

### Threads and composer

| Today                                                                                                               | Token                                                                                                                                                                            |
| ------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Block `MARGIN_X` 10, `MARGIN_Y` 6, `AVATAR` 20, radius 6                                                            | `NESTED_X` 12, `NESTED_Y` 8 (the block's offset in its card), `AVATAR_SM`, radius `MD` (bug: cards are 8)                                                                        |
| Header row 30 `px_2p5`; resolved row `min_h` 30 `px_3 py_1`                                                         | `MD`, `COMPACT_X`; `MD`, `COMPACT_X` / `INLINE`                                                                                                                                  |
| Comment `px_3 py_2 gap_1`; body `pl` AVATAR + 8                                                                     | `COMPACT_X` / `COMPACT_Y` / `INLINE`; `AVATAR_SM` + `CONTROLS`                                                                                                                   |
| Badge `px_1p5` at line height 16                                                                                    | `BADGE_X` with `text::SMALL`                                                                                                                                                     |
| Snippet line height 1.54 ×; gutter formulas (+10, +12); suggestion `my_1` r6                                        | The code-row rule; `card::gutter` from the snippet frame's inner edge, then `CODE_PAD` (bug: duplicates; it does not align with the diff); `INLINE`; radius `SM`                 |
| Threads panel header 40 `pl_3 pr_2 gap_1`                                                                           | `BAR` 36; S1/S2 rules; `INLINE`                                                                                                                                                  |
| Panel list `px_3 pt_1 pb_3 gap_2`; empty `pt_2`                                                                     | `SIDEBAR`, `INLINE`, `GROUP`, `CONTROLS`; `SECTION` (bug: other empty states use 24)                                                                                             |
| Panel cards r8 / inner 7, `px_3 py_2 gap_1`; selection rail left 3, inset 6, w 2 or 3 by focus; wrapper `px_2 pb_2` | `MD` / `MD − BORDER`; `COMPACT_X` / `COMPACT_Y` / `INLINE`; rail at `RIM`, inset `CONTROLS`, `stroke::CURSOR_BAR` 2 at any focus (bug: its width changes with focus); `CONTROLS` |
| Filter buttons 24 `px_1p5` r6; widths 340 (220–720); travel 12                                                      | `SM`, `ICON_LEAD`, radius `SM`; app tokens; retired with `enter_from` (T7.10)                                                                                                    |
| Composer tabs 24 `px_2p5` r5                                                                                        | `SM`, `PILL_X`, radius `SM` (bug: r5)                                                                                                                                            |
| Editor `min_h` 64 `px_3 py_2` around `px_1 py_1`                                                                    | `COMPOSER_MIN_H`; `COMPACT_X` / `COMPACT_Y`; the inner padding deleted (bug)                                                                                                     |
| Toolbar 34 `px_1p5 gap_2 pr_1p5`; footer `px_2p5 py_2 gap_2`                                                        | `BAR` 36 (bug: off the ladder); 6 / 8 / 6; 12 / 8 / 8                                                                                                                            |
| Frame and focus r6; collapsed input 28 `px_2p5`; reply rows 12/8 vs 8/8                                             | radius `MD` (a card); `MD`, `TEXT`; both 12 / 8 (bug)                                                                                                                            |

### Home and overlays

| Today                                                                                 | Token                                                                                                             |
| ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| Home page `px_8 pt_6 pb_10`, cards `gap_2`, `max_w` 1120, section `pt_4`              | `PAGE`, `SECTION`, `PAGE` (bug: 40); `CARDS` 12; `HOME_MAX_W`; `SECTION`                                          |
| Home card 60, r8, `pl_4 pr_2 gap_3`; accent bar left 4, w 3, h 28                     | `HOME_CARD`; `MD`; `CARD_X`, `CARD_TRAILING`, `GROUP`; `INLINE`, `stroke::CHANGE_BAR` 4, `MD`                     |
| Kind badge `px_1p5` r6 and status `px_2 py_0p5` capsule; title 15; five column widths | One badge: `XS` 20 capsule, `BADGE_X` (bug: two shapes); `text::TITLE`; `HOME_COL_*`                              |
| Pickers w 560 at `mt` 96; `max_h` 380 / 400 / 420                                     | `PICKER_W`, `PICKER_TOP`; one `OVERLAY_MAX_H` 400 (bug)                                                           |
| Open flow 640 × 460, cheat sheet 1040, both at `mt` 72                                | App tokens; `DIALOG_TOP`                                                                                          |
| Rows: finder 30, base picker 44, repo 46, commit 48, cheat sheet 26, find 24          | `MD`; `ROW2` 44 ×3; `SM`; `SM`                                                                                    |
| Headers `px_3 pt_3 pb_1` vs `px_3 pt_2 pb_1`; empty `py_6`                            | One overlay header: x `CARD_X` (= `OVERLAY` + `TEXT`), `pt` `GROUP`, `pb` `INLINE` (bug: two variants); `SECTION` |
| `theme.radius` 6, `radius_lg` 10, literal 6 in Submit, key caps `radius.half()` 3     | radius `SM`; `LG` 12; `SM`; `XS` 4                                                                                |
| Submit `gap_2p5`, `px_2p5`, `pt(1)`, size 7                                           | `GROUP`; `GROUP`; 0; `DOT` (bug: off-scale)                                                                       |
| Kit menu 8 pt margin                                                                  | Kit internal; outside the lint                                                                                    |

### Values first missed, and their owners

Each value the scanner finds is migrated by one named task (plan M7, Spacing ownership).

| Today                                                                                                                         | Token                                                                                                                                             | Task              |
| ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------- |
| `PILL_TEXT_MAX` 64, `REPO_NAME_MAX` 48 (`review_tab/toolbar.rs:85,87`)                                                        | `layout::PILL_TEXT_MAX`, `layout::REPO_NAME_MAX`                                                                                                  | T7.5              |
| Toolbar repo block `line_height(relative(1.2))`                                                                               | `text::UI` over `text::SMALL`                                                                                                                     | T7.5              |
| `N/M` `.px_1().text_sm()` (`viewed/mod.rs:477-478`)                                                                           | 0 (the cluster's `CONTROLS` separates it); `text::UI`, tabular figures                                                                            | T7.5              |
| Filter menu `max_h(px(420.))` (`tree/filters.rs:256`)                                                                         | `layout::OVERLAY_MAX_H`                                                                                                                           | T7.5              |
| Tree `SLOT` 20 (`tree/row.rs:39`)                                                                                             | `layout::VIEWED_SLOT` (OQ-43)                                                                                                                     | T7.5              |
| Key cap `line_height(relative(1.))`                                                                                           | `text::CAPTION`                                                                                                                                   | T7.6              |
| `THREADS_WIDTH` 340, `THREADS_MIN_WIDTH` 220, `THREADS_MAX_WIDTH` 720, `VIEWPORT_MIN_WIDTH` 260 (`review_tab/panes.rs:30-36`) | `layout::{THREADS_WIDTH, THREADS_RANGE, VIEWPORT_MIN_WIDTH}`                                                                                      | T7.7              |
| `px(100_000.)` as "unbounded" (`review_tab/panes.rs:204`)                                                                     | `Pixels::MAX` (as the shell)                                                                                                                      | T7.7              |
| Header card `line_height(px(18.))`, `(px(17.))`                                                                               | `text::TITLE`, `text::BODY`                                                                                                                       | T7.7              |
| Block badge `line_height(px(16.))`; snippet `((size − 1) · 1.54).round()`                                                     | `text::SMALL`; the code-row rule                                                                                                                  | T7.7              |
| Blank markdown line `min_h(rems(1.2))` (`markdown/sanitize.rs:420`)                                                           | `text::BODY`'s line height                                                                                                                        | T7.7              |
| Suggestion `px((size · 1.54).round())`                                                                                        | The code-row rule                                                                                                                                 | T7.7              |
| `ENTER_FROM_ABOVE` 4 (`review_tab/banners.rs`)                                                                                | Motion's `NUDGE`                                                                                                                                  | T7.9              |
| `ENTER_FROM_RIGHT` 12 (`review_tab/panes.rs:38`)                                                                              | Retired with `enter_from`                                                                                                                         | T7.10             |
| Assign dialog `.w(px(520.))` (`home/dialogs.rs:100`)                                                                          | `layout::DIALOG_W`                                                                                                                                | T7.6              |
| Iteration picker `min_w(px(280.))` (`iterations/picker.rs:173`)                                                               | `layout::MENU_MIN_W`                                                                                                                              | T7.6              |
| Open-flow radio `mt(px(2.))`, `size(px(14.))`, `p(px(3.))` (`open_flow/source_step.rs:830-836`)                               | Centred on its label's first line, `(text::UI line height − ICON_SM) / 2`; `ICON_SM`; its dot centred, `(ICON_SM − DOT) / 2` (all derived)        | T7.6              |
| `.xsmall()` Buttons at the kit radius 6 (about 20 call sites)                                                                 | `.rounded(px(radius::XS))` (`for_height(20)`)                                                                                                     | Each file's owner |
| `CardStyle::default`, `Metrics::default` fields; painter arithmetic above                                                     | `space` tokens; `radius + 2.0` (a clip reach) → `radius + stroke::FOCUS_RING`; `0.5 · a` → the `card` columns; `.clamp(0.0, 4.0)` → `gap::INLINE` | T7.4              |
