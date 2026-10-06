# ADR-0027: Polygloss themes and the card layout

- **Status:** Accepted. Supersedes ADR-0024's default themes (Pierre Light/Dark stay bundled and selectable); the rest of ADR-0024 stands. The card numbers (16 pt sides, a 2.25-row file header) are amended by [ADR-0031](0031-spacing-system.md).
- **Date:** 2026-10-05
- **Design:** [§11.6 Diff viewport](../design.md#116-diff-viewport-adr-0003), [§11.10 Themes and fonts](../design.md#1110-themes-and-fonts-adr-0024), [research: redesign reference](../research/redesign-reference.md)

## Context

The redesign follows a light reference screenshot: a gray canvas, one rounded white card per file, a colored bar at the left edge of each changed row instead of `+`/`-` glyphs, tinted line numbers, a monochrome chrome and a header card for the commit. Its colors were sampled from the image ([research](../research/redesign-reference.md)); no dark reference exists. Some of its syntax colors are below 4.5:1 on white. SF Mono is reachable on stock macOS only through the private family name `.AppleSystemUIFontMonospaced`; Apple's license forbids bundling it.

## Decision

- **Default themes:** new **Polygloss Light** and **Polygloss Dark** (Zed theme JSON in `assets/themes/`, our own palette). Light follows the sampled colors; dark keeps the same structure with chosen values. Strings, types and comments are darkened from the sample to reach about 4.3–4.7:1 (comments 3.6:1). Pierre Light/Dark stay bundled.
- **Theme keys:** standard Zed keys first. Colors Zed has no key for use `polygloss.*` style keys (sidebar field, changed-line numbers and gutters, stat colors, commit SHA); every one has a fallback derived from standard keys, so any Zed theme still renders.
- **Cards:** each file is a rounded card (radius 8, 1 px border) on the canvas, 12 px apart, 16 px from the sides. The sticky header stays inside its card and pins flush and square at the top edge; it keeps every v1 badge (rename similarity, mode, kind, generated, LFS, changed since viewed, open threads, agent) as pills. A header card (commit, compare or live) sits above the first file and scrolls with the diff. The scroll anchor gains a top-of-lead key, so a review opens at the top of the header card and stays there while the card loads or its commit list grows.
- **Rows:** `diff.style.indicators` defaults to `"bars"`: a 3 px bar at the left edge of each changed row (each half's own edge in split), no glyph; changed rows tint their line numbers and gutter. `"+-"` and `"none"` stay selectable. An added or deleted text file renders as one full-width pane with one number column in both layouts, as the reference's added files do (provisional, design OQ-54).
- **Fonts:** code keeps bundled **Lilex** for now (provisional, design OQ-40: the user decides at the M6 gate from a side-by-side screenshot; the alternative is the system mono by default with Lilex as fallback and in baselines). `"SF Mono"` and `"System Mono"` become aliases of the system monospaced family, which also replaces Menlo as the missing-font fallback. UI: the system font.
- **Icons:** Lucide icons from gpui-kit-assets, embedded selectively (`icon_assets!`), never the whole catalog.
- **Avatars:** an initial on a color picked by FNV-1a of the lowercased author email; never fetched.

## Consequences

- Every screenshot baseline changes; the M6 gate re-records all of them and compares them with the reference.
- The viewport paints cards, rounded per-corner quads, SVG icons and a second (UI) font, so frame cost rises slightly. Every budget must still hold with scroll p95 under a quarter of its budget; a `--compare-baseline` regression is re-baselined only with the numbers recorded in the M6 gate.
- The perf harness keeps Pierre Light so its numbers stay comparable.
- `NOTICE` needs no new entry for the palette: no files were copied.

## Alternatives rejected

| Option                                 | Why not                                                                                                                                               |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| Restyle Pierre in place                | Pierre is a port of a published theme; users who chose it should keep it                                                                              |
| Exact sampled syntax colors            | Strings 2.9–3.3:1, types 3.8:1 and comments 3.2:1 on white                                                                                            |
| SF Mono as the default code font       | Not rejected, deferred (OQ-40): reachable only by a private family name whose metrics may change with macOS releases; baselines could still pin Lilex |
| Bundling SF Mono                       | Apple's license                                                                                                                                       |
| Gravatar or other remote avatars       | Network access (ADR-0005)                                                                                                                             |
| gpui-component `Avatar`                | Its color hashes the initials, not the email                                                                                                          |
| A fixed header card above the viewport | Costs about 70 px of height for the whole review; the reference scrolls it with the diff                                                              |
