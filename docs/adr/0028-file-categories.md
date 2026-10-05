# ADR-0028: File categories

- **Status:** Accepted
- **Date:** 2026-10-05
- **Design:** [§11.15 File categories](../design.md#1115-file-categories-adr-0028), [§7.2 Schema](../design.md#72-schema-v1), [§18 Settings](../design.md#18-settings-and-keymap-files)

## Context

Agent changes mix the work with tests, lockfiles, generated code, docs and agent config. [geld](https://github.com/brandonmcconnell/geld) (MIT), a browser extension for GitHub, moves such files out of the main list by path patterns, per configurable category. We want the same, configurable in `settings.json`, without changing any id: `file_changes.idx` is git's order and keys the store, the MCP server and the viewport.

Two facts constrain it. Stored file lists are frozen per `diff_id`, while settings hot-reload. And today's `file_changes.generated` folds the `linguist-generated` attribute and a built-in list into one bit, so an explicit `-linguist-generated` cannot be told apart from "not in the list"; `diff.generated_patterns` was parsed but never applied.

## Decision

- **Classifier in core:** `polygloss_core::categories` is pure (no git, IO or GPUI) and shared by the app, the MCP server and the CLI. It uses `globset` (already in the shipped graph). Categories are computed at view time from the path, the stored attribute and the settings, and never stored.
- **Patterns** (gitignore-like, geld's rules except the leading `/` and the trailing slash): braces expand first and each alternative is read on its own; no slash = any depth; a trailing slash = a directory of that name at any depth and everything in it, never a file or submodule of that name, as in gitignore (deviation from geld, whose tests pin `tests/` matching the file `tests` and so put `scripts/test` scripts in Tests; OQ-45); a leading `/` anchors at the repo root; `*` and `?` stay in one segment, `**` spans segments; `[…]` classes; a leading `!` rescues the path from that category only (matching goes on with the next category). A `{` without its `}` is a literal, as in geld; `\` escapes the next character.
- **Order:** custom categories (settings order), then an explicit `linguist-generated` → Generated, then Tests, Generated, Vendored, Agent config, Docs, Tooling & CI, Stories & fixtures. The first match wins; disabled categories are skipped. The attribute is the repo's statement about one file, so it beats the pattern lists both ways: set → Generated at the attribute step, whatever Generated's rescues say; unset → never Generated (both Generated steps are skipped; later categories still apply). With Generated disabled, a set file goes on to Tests and the rest. Git's `true` and `false` values count as set and unset, as v1 read them. Rows from before v2 (attribute unknown) recover it: v1 stored "the attribute if specified, else its 15-entry built-in list", so bit 0 on a listed path is Unset, bit 1 on an unlisted path is Set, and the two ambiguous cases are Unspecified. That list is frozen in core for this purpose.
- **Defaults:** Tests and Generated on, the rest off. Generated's `build-output` group (`dist/`, `build/`, `out/`) is off by default. Built-in pattern lists are ported from geld at commit `5b8ce0e`, credited in `NOTICE`.
- **Generated stays one notion:** a file is generated when its attribute is set, or when it is unspecified and the Generated patterns match (enabled groups, extras, and `diff.generated_patterns`, kept as an alias; minus rescues). That drives "Load diff" whether or not the category is enabled. `diff.generated_patterns` now uses these pattern rules; a legacy value that does not compile is logged and dropped instead of invalidating `settings.json`.
- **Store v2:** `file_changes.generated_attr` keeps the attribute's tri-state (unspecified, set, unset; `NULL` for rows written before v2). The `generated` column is still written as before.
- **Display:** the partition is computed synchronously when a review tab attaches, so the first frame already has its sections; hot reloads re-partition in the background and apply only the latest load's result. Refresh and iteration switches re-partition before the next frame, keeping each section's open state by category. Files of an enabled category leave the main list and tree. They go to the bottom of the diff, one collapsible section per category; the sidebar's Files segment becomes an accordion of trees. A section starts closed unless every file is categorized or one of its files holds an agent question waiting on you; a saved state wins. Its band and its sidebar panel show the open-thread count, an agent badge and the changed-since-viewed dot of its files. Header and footer totals exclude categorized files; Viewed progress counts every file.
- **Navigation:** stepwise keys (`j`/`k`, `]`/`[`, `n`/`p`, and the jump to the next unviewed file after `v`) walk only shown files and pass over closed sections. Explicit targets (a tree row, ⌘P, Find, a thread, a URL, MCP `focus`) open the section first. Anything else that would leave the anchor in a hidden file (closing its section, a settings change, a restored view state) moves it to that section's band.
- **The viewport stays category-agnostic:** the app hands it sections (a label and a list of file indices). It keeps a display order internally; every API stays keyed by `file_idx`. Replacing its provider clears sections and order (the app re-partitions). The app computes each file's Generated verdict before it builds a provider, so opening a review never swaps providers; a hot reload that changes verdicts relabels those files in place, keeping every other file's rows, collapse state, flags, threads, cursor and anchor.
- **Agents:** `open_diff` files and the diff resource gain a `category` computed from `settings.json` (the app's per-tab palette toggles are invisible to agents); `polygloss debug categorize` explains a verdict. Threads, anchors and `focus` are path-based and unchanged.

## Consequences

- One store migration (v1 → v2) with the usual backup.
- Hidden files cost nothing per frame: the viewport walks visible files by offset, so a collapsed section of 10,000 files is skipped in O(log n).
- Agent threads in a closed section stay visible: on its band, its sidebar panel, the threads button and the agent-replies banner, whose jump opens the section.
- If `settings.json` is invalid, the app keeps its last good categories while `polygloss mcp` uses the defaults until the file is fixed.
- Palette toggles change one review tab for the session; `settings.json` is the persistent route (no settings writer exists).

## Alternatives rejected

| Option                                                 | Why not                                                                                                                                                             |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Store categories in `file_changes`                     | Frozen per diff, while settings hot-reload                                                                                                                          |
| Reorder the host's file list (rewrite `idx`)           | Desyncs store, MCP, blocks and flags keyed by `idx`; every re-partition would remap app state                                                                       |
| Classify in the app only                               | The MCP server and CLI need the same verdicts                                                                                                                       |
| `ignore`'s gitignore matcher                           | Last match wins; geld's per-category rescues need first-match with scoped negation                                                                                  |
| `gix-glob`                                             | No brace alternatives                                                                                                                                               |
| A v1 row's bit as one more Generated pattern           | A listed file with `-linguist-generated` (shown in full in v1) would become Generated, and a set `a.test.ts` would land in Tests while its v2 row goes to Generated |
| No migration (stored bit OR patterns)                  | An explicit `-linguist-generated` would lose to the larger lists, and unticking lockfiles could not un-generate `Cargo.lock`                                        |
| Change kinds as categories (geld's "trivial", "large") | Not path-based; large files already collapse behind "Load diff"                                                                                                     |
