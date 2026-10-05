# Redesign reference (M6)

Findings behind [ADR-0026](../adr/0026-inset-titlebar-and-sidebar-navigation.md)–[ADR-0029](../adr/0029-product-motion.md) and plan M6, gathered 2026-10-05. The reference is a light-mode screenshot of a third-party diff app (godiff), supplied by the user. It is **not** in the repo and must not be added: only measurements and sampled colors are recorded here.

## Anatomy

| Area        | Reference                                                                                                                                                                                                                                          |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Window      | No titlebar, no tab row. Traffic lights at the top left of a full-height sidebar.                                                                                                                                                                  |
| Sidebar top | Traffic lights, then (right-aligned) a 2-segment icon control (file tree, history) and a sidebar-toggle icon.                                                                                                                                      |
| Sidebar     | A rounded gray "Filter files" field; the tree (chevrons, outline folder/file icons, proportional UI font, right-aligned monospace `+430 −0` in green/red, a colored status letter); footer `Total: +7,726 −0`.                                     |
| Toolbar row | Same height as the sidebar's top row. Repo name bold with its parent path dim below; capsule pills (branch icon + `main`; commit icon + SHA in orange monospace). Right: search icon, comment icon with a count, a 2-segment split/unified toggle. |
| Canvas      | Very light gray. A white header card: circular avatar with an initial, commit subject bold, "<author> committed just now", short SHA right-aligned in orange monospace.                                                                            |
| File card   | Rounded white card, 1 px border, gaps between cards. Header: collapse chevron, path in monospace with the directory dim and the file name bold, an open-external icon, a `+430 −0` pill, a "Viewed" pill button with a checkbox.                   |
| Diff rows   | Added rows on soft green with a solid green bar at the row's left edge and green line numbers on a slightly stronger green gutter; no `+` glyphs. Monospace similar to SF Mono; generous line height.                                              |

## Measurements

From the 2× capture, in points: sidebar ≈ 280 wide; top rows ≈ 51 tall; code rows 20; tree rows ≈ 29; file header ≈ 45; card radius 7–8; gap between cards ≈ 12; canvas padding ≈ 13; close button centre ≈ (25.8, 24.5) from the window's top left, so a 14 pt button sits at about (19, 18).

## Sampled colors

Method: the PNG's pixels decoded (sRGB chunk); each surface is the mode of a region; each text or glyph color the most common pixel among the 3% most different from its background.

| Element                                                                         | Color                                                                 |
| ------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Sidebar (translucent: sky shows through)                                        | `#dbe9ea` top, `#e7e9ea` lower and footer                             |
| Sidebar footer border / divider to main                                         | `#d1d3d4` / `#c8d3d4`                                                 |
| Filter field, segmented track                                                   | `#d1dcdd` (≈ 6% black over the sidebar)                               |
| Selected segment                                                                | `#ffffff`                                                             |
| Toolbar / its bottom border                                                     | `#fcfcfb` / `#e4e4e3`                                                 |
| Canvas / cards / card border                                                    | `#f8f8f6` / `#ffffff` / `#e7e7e7`                                     |
| Pills (branch, commit, ±)                                                       | `#f2f2f1`, `#f1f1f1`                                                  |
| Viewed pill / checkbox outline                                                  | white, border `#e3e3e3` / `#c5c5c5`                                   |
| Added row / gutter / bar and number                                             | `#e6f6eb` / `#d9f2e0` / `#57bb5c`                                     |
| Stats text added / deleted                                                      | `#3c7849` / `#aa3c36`                                                 |
| Commit SHA / avatar fill                                                        | `#b9722c` / `#b25e7d`                                                 |
| Text / dim text and icons                                                       | `#18181b` / `#717179`                                                 |
| Syntax: keyword / string / builtin type / comment / identifiers / function name | `#342dda` / `#48a04c` / `#b9722c` / `#919191` / `#111111` / `#2e407d` |

No deleted rows or word highlights appear; those colors below are derived.

## Polygloss palette

Contrast ratios against white were measured; strings, orange and comments were darkened keeping their hue: string `#3a8a3f` (4.3:1), orange `#a8621f` (4.7:1), comment `#86868b` (3.6:1), added line number `#3f9a45` (3.0:1 on its gutter). Dark values are chosen, not sampled (no dark reference); all its text pairs measure 4.2–15:1.

| Zed key                                                                          | Light                                                             | Dark                                                              |
| -------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ----------------------------------------------------------------- |
| `background` (canvas)                                                            | `#f8f8f6`                                                         | `#111113`                                                         |
| `editor.background`, `surface.background`, `editor.subheader.background` (cards) | `#ffffff`                                                         | `#18181b`                                                         |
| `elevated_surface.background` (popovers)                                         | `#ffffff`                                                         | `#202023`                                                         |
| `panel.background` (sidebar, opaque)                                             | `#ebebea`                                                         | `#1c1c1f`                                                         |
| `title_bar.background`, `toolbar.background`                                     | `#fcfcfb`                                                         | `#161618`                                                         |
| `border` / `border.variant`                                                      | `#e7e7e7` / `#e4e4e3`                                             | `#2a2a2e` / `#27272a`                                             |
| `element.background` (pills) / `element.hover`                                   | `#f2f2f1` / `#ebebea`                                             | `#27272a` / `#2f2f33`                                             |
| `ghost_element.hover` / `.selected`                                              | `#0000000a` / `#0000000f`                                         | `#ffffff0d` / `#ffffff12`                                         |
| `text` / `text.muted` / `text.placeholder`                                       | `#18181b` / `#6b6b73` / `#71717a`                                 | `#ececed` / `#a1a1aa` / `#8b8b93`                                 |
| `editor.foreground` / `editor.line_number`                                       | `#111111` / `#8e8e96`                                             | `#e4e4e7` / `#6b6b73`                                             |
| `text.accent`, `border.focused`                                                  | `#1f6feb`                                                         | `#4c8dff`                                                         |
| `created` / `created.background`                                                 | `#57bb5c` / `#e6f6eb`                                             | `#4cc35a` / `#1b2a1f`                                             |
| `deleted` / `deleted.background`                                                 | `#e5605a` / `#fdeded`                                             | `#f0605a` / `#2d1c1d`                                             |
| `version_control.word_added` / `.word_deleted`                                   | `#bfe8c8` / `#f7c9c6`                                             | `#2c5434` / `#5c2b2b`                                             |
| `version_control.added` / `.modified` / `.deleted` / `.renamed` (status letters) | `#3f9a45` / `#b7791f` / `#c4433c` / `#7553c9`                     | `#5fcf6b` / `#e0a526` / `#ff7b72` / `#a48bf0`                     |
| `players[0..8].cursor` (avatar colors)                                           | `#1f6feb #4f74b8 #3f8f62 #b07a3a #7a5cc0 #2f8a9c #b8634f #6b7280` | `#4c8dff #7fa2e6 #6cc08f #d9a465 #a68be6 #5cc1d6 #e08b77 #9ca3af` |

`polygloss.*` keys (Zed ignores them; our parser keeps unknown keys). Each has a fallback, so themes without them still work:

| Key                                   | Light                 | Dark                  | Fallback                                           |
| ------------------------------------- | --------------------- | --------------------- | -------------------------------------------------- |
| `polygloss.sidebar.field.background`  | `#dcdcdb`             | `#2a2a2e`             | `element.background`                               |
| `polygloss.created.line_number`       | `#3f9a45`             | `#5fcf6b`             | `created`                                          |
| `polygloss.created.gutter_background` | `#d9f2e0`             | `#1f3524`             | `created.background`, slightly stronger            |
| `polygloss.deleted.line_number`       | `#c4433c`             | `#ff8a80`             | `deleted`                                          |
| `polygloss.deleted.gutter_background` | `#fbe1df`             | `#3d2224`             | `deleted.background`, slightly stronger            |
| `polygloss.stat.added` / `.deleted`   | `#3c7849` / `#aa3c36` | `#6bd17a` / `#ff7b72` | `version_control.added` / `.deleted`               |
| `polygloss.commit_sha`                | `#a8621f`             | `#e3a25b`             | syntax `type.builtin`, then `terminal.ansi.yellow` |

Syntax (light / dark): `keyword`, `constant.builtin`, `boolean`, `variable.builtin`, `tag`, `punctuation.special`, `markup.heading`, `markup.link` `#342dda` / `#a39dff`; `string`, `character`, `markup.raw` `#3a8a3f` / `#7ccf7f`; `string.escape`, `string.regexp` `#2a8a9e` / `#5cc1d6`; `type.builtin`, `number`, `attribute`, `label`, `tag.attribute`, `function.builtin`, `markup.list` `#a8621f` / `#e3a25b`; `comment`, `markup.quote` `#86868b` / `#85858c`; `function`, `function.macro`, `constructor`, `markup.link.url` `#2e407d` / `#9fb4f0`; everything else (`type`, `variable`, `property`, `punctuation`, `operator`, `constant`, `module`) `#111111` / `#e4e4e7`; `tag.delimiter` `#6b6b73` / `#a1a1aa`.

Fallback avatar hues for themes with fewer than 8 players (Pierre): light `#b25e7d #4f74b8 #3f8f62 #b07a3a #7a5cc0 #2f8a9c #b8634f #6b7280`; dark the dark players row above.

## Fonts

- `.AppleSystemUIFontMonospaced` resolves through GPUI's family lookup to 12 SF Mono faces on stock macOS (CoreText probe, and `tests/e2e/kit_fonts.rs` asserts it). It is a private dot-prefixed name.
- `"SF Mono"` by name resolves only where Apple's developer fonts are installed in `/Library/Fonts`; `.SF NS Mono` fails.
- Lilex at 13 pt gives 20 pt rows (`round(1.5 × size)`), the reference's rhythm.

## Icons

gpui-kit-assets 0.7.0 ships all 1,818 Lucide 1.43 icons; the default bundle embeds 101. `icon_assets!` embeds a selection (≈ 20–40 KiB for 30 icons; `AllAssets` is 1 MiB). Lucide is ISC/MIT and already credited in `packaging/third-party-notices.md`. An icon not registered draws nothing, silently. Lucide 1.43 has no `history`; `rotate-ccw-clock` is the closest.

| Use                               | Icon                                                                                                                                                                            |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Sidebar segments: Files / Reviews | `list-tree` / `rotate-ccw-clock`                                                                                                                                                |
| Sidebar toggle                    | `panel-left`, `panel-left-open`                                                                                                                                                 |
| Tree                              | `folder` (expanded or not, as in the reference), `file`, `chevron-right`, `chevron-down`, `circle`, `circle-check`, `circle-minus` (partly viewed folder)                       |
| Toolbar pills                     | `git-branch`, `git-commit-horizontal`, `git-compare`, `circle-dot` (live)                                                                                                       |
| Toolbar buttons                   | `search`, `message-square`, `columns-2`, `rows-2`, `sliders-horizontal`                                                                                                         |
| File card                         | `square-arrow-out-up-right` (open in editor), `ellipsis`, `square`, `square-check`; review-state pills `message-square`, `bot`                                                  |
| Header card                       | `camera` (Snapshot)                                                                                                                                                             |
| Categories                        | Tests `flask-conical`, Generated `file-cog`, Vendored `package`, Agent config `bot`, Docs `book-open`, Tooling & CI `wrench`, Stories & fixtures `layers`; custom default `tag` |
| Badges                            | `file-symlink`, `binary`, `hard-drive` (LFS), `folder-git` (submodule), `dot`                                                                                                   |

## GPUI capabilities checked

| Capability                    | Finding (gpui-pre 0.3.7, gpui-kit 0.7.0)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Traffic lights                | `TitlebarOptions.traffic_light_position`, `Window::set_traffic_light_position`; AppKit restores them in fullscreen.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Dragging                      | `app_owns_titlebar_drag`, `Window::start_window_move`, `titlebar_double_click` (honors AppleActionOnDoubleClick). gpui-component `TitleBar` implements both and can be restyled; two instances need parents with distinct ids. Buttons do not stop mouse-down.                                                                                                                                                                                                                                                                                                                                                             |
| Blur                          | `WindowBackgroundAppearance::Blurred`: one NSVisualEffectView behind the whole window, Selection material, non-opaque Metal layer; no per-region vibrancy, no Reduce Transparency read, not visible headless.                                                                                                                                                                                                                                                                                                                                                                                                              |
| Native tabs                   | `tabbing_identifier: None` disables them; each native tab would be its own `NSWindow`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Motion                        | `Animation::new(..).with_easing(..)`, `with_animation(id, ..)` (wall clock, `Instant::now()`); gpui-base `animate_keyframes`, `transition`, `presence` (executor clock: `TestAppContext` steps it with `advance_clock`); `ease_out_quint`, `ease_out_cubic`. Under `App::reduce_motion()` they show the end state at once. No scale or transform on `div`s, no group opacity (a fading element's shadow shows through). gpui-component's dropdown motion (`dropdown_popup`, 150 ms, 8 px) is crate-private and serves Select, Combobox and DatePicker; `DropdownMenu` has none, and its `PopupMenu` paints its own shadow. |
| Reduce Motion                 | Read once at init (`gpui_base::reduce_motion`); `gpui_kit::base::apply_system_reduce_motion` re-reads it. Setting it from the app stops the system follow.                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Painting                      | `Window::paint_svg`; `quad()` takes per-corner radii; `ContentMask` is rectangular, so rounded corners cannot clip rows.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Motion inherited from the kit | Dialog 250 ms entrance (no per-dialog switch), toasts 400 ms in, tooltips 150 ms, scrollbars fade, `TabBar` spring, `Accordion`/`Collapsible` height spring, `Sidebar` width, `Checkbox` mark spring.                                                                                                                                                                                                                                                                                                                                                                                                                      |

## geld

[brandonmcconnell/geld](https://github.com/brandonmcconnell/geld), MIT ("Copyright (c) 2026 Brandon McConnell"), read at commit `5b8ce0e470fcd2b2538da6c700d7d96197f57de3`, `packages/core/src/`:

- `categories.ts`, `test-patterns.ts`: built-in categories in match order (tests on by default; generated, vendored, agents, docs, tooling, stories off) with pattern groups: tests/{unit, e2e, directories, snapshots, tooling}, generated/{lockfiles, generated-code}, vendored/{vendored}, agents/{agents}, docs/{docs}, tooling/{ci, lint-format, build-config}, stories/{stories, fixtures, i18n}. Also "trivial" and "large", which are change kinds, not paths.
- `glob.ts`: no slash = basename at any depth; a trailing slash = `**/dir` and its contents (even with inner slashes); a leading `/` is stripped (any depth); `*`/`?` within a segment, `**` whole segments; nested `{a,b}`; `[…]` classes.
- `matcher.ts`: custom categories first, then built-ins; per category a user `!rescue` skips that category only and matching continues; built-in groups, then user extras; `explain()` returns the category, pattern and source, or `rescuedBy`.
- Pattern counts per group (the drift guard for the port): tests/unit 19, tests/e2e 17, tests/directories 33, tests/snapshots 9, tests/tooling 52, generated/lockfiles 22, generated/generated-code 25 (22 after `dist/`, `build/`, `out/` move to `build-output`), vendored 11, agents 29, docs 19, tooling/ci 26, tooling/lint-format 31, tooling/build-config 35, stories/stories 5, stories/fixtures 9, stories/i18n 15.
- Nouns (`noun`/`nounPlural`, `shortNoun`/`shortNounPlural`): design §11.15's table. A custom category's nouns default to its lowercased title.
- Test vectors: `glob.test.ts` (96 lines) and `matcher.test.ts` (220 lines) give path → verdict pairs. Not ours to port: case-insensitive matching, `./` and backslash normalization, a leading `/` (geld strips it; OQ-45 anchors), repo-scoped `[owner/repo]` patterns, change kinds (trivial, large), and cases that reach `dist/`, `build/`, `out/` only through `build-output` (port them with that group enabled).
- `settings.ts`: per-category booleans, `"cat/group": false`, per-category extra patterns, custom categories with `custom:<slug>` ids.

Already in the shipped graph: `globset` 0.4.20 (Unlicense OR MIT; nested braces, classes, `literal_separator`, byte candidates, every matching pattern index). `ignore`'s gitignore matcher is last-match-wins; `gix-glob` has no braces.
