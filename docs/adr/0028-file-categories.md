# ADR-0028: File categories

- **Status:** Accepted
- **Date:** 2026-10-05
- **Design:** [§11.15 File categories](../design.md#1115-file-categories-adr-0028), [§7.2 Schema](../design.md#72-schema-v1), [§18 Settings](../design.md#18-settings-and-keymap-files)

## Context

Agent changes mix the work with tests, lockfiles, generated code, docs and agent config. [geld](https://github.com/brandonmcconnell/geld) (MIT), a browser extension for GitHub, moves such files out of the main list by path patterns, per configurable category. We want the same, configurable in `settings.json`, without changing any id: `file_changes.idx` is git's order and keys the store, the MCP server and the viewport.

Two facts constrain it. Stored file lists are frozen per `diff_id`, while settings hot-reload. And today's `file_changes.generated` folds the `linguist-generated` attribute and a built-in list into one bit, so an explicit `-linguist-generated` cannot be told apart from "not in the list"; `diff.generated_patterns` was parsed but never applied.

## Decision

- **Classifier in core:** `polygloss_core::categories` is pure (no git, IO or GPUI) and shared by the app, the MCP server and the CLI. It uses `globset` (already in the shipped graph). Categories are computed at view time from the path, the stored attribute and the settings, and never stored.
- **Patterns** (gitignore-like): no slash = any depth; a trailing slash = that directory at any depth and everything in it; a leading `/` anchors at the repo root; `*` and `?` stay in one segment, `**` spans segments; `{a,b}` alternatives; `[…]` classes; a leading `!` rescues the path from that category only (matching goes on with the next category).
- **Order:** custom categories (settings order), then an explicit `linguist-generated` → Generated, then Tests, Generated, Vendored, Agent config, Docs, Tooling & CI, Stories & fixtures. The first match wins; disabled categories are skipped.
- **Defaults:** Tests and Generated on, the rest off. Generated's `build-output` group (`dist/`, `build/`, `out/`) is off by default. Built-in pattern lists are ported from geld at commit `5b8ce0e`, credited in `NOTICE`.
- **Generated stays one notion:** a file is generated when its attribute is set, or when it is unspecified and the Generated patterns match (enabled groups, extras, and `diff.generated_patterns`, kept as an alias). That drives "Load diff" whether or not the category is enabled.
- **Store v2:** `file_changes.generated_attr` keeps the attribute's tri-state (unspecified, set, unset; `NULL` for rows written before v2). The `generated` column is still written as before.
- **Display:** files of an enabled category leave the main list and tree. They go to the bottom of the diff, one collapsible section per category, collapsed unless every file is categorized; the sidebar's Files segment becomes an accordion of trees. Header and footer totals exclude them; Viewed progress counts every file.
- **The viewport stays category-agnostic:** the app hands it sections (a label and a list of file indices). It keeps a display order internally; every API stays keyed by `file_idx`.
- **Agents:** `open_diff` files and the diff resource gain a `category`; `polygloss debug categorize` explains a verdict. Threads, anchors and `focus` are path-based and unchanged.

## Consequences

- One store migration (v1 → v2) with the usual backup.
- Hidden files cost nothing per frame: the viewport walks visible files by offset, so a collapsed section of 10,000 files is skipped in O(log n).
- Jumping to a file in a collapsed section (Find, threads, URLs, `focus`) opens the section first.
- If `settings.json` is invalid, the app keeps its last good categories while `polygloss mcp` uses the defaults until the file is fixed.
- Palette toggles change one review tab for the session; `settings.json` is the persistent route (no settings writer exists).

## Alternatives rejected

| Option                                                 | Why not                                                                                                                      |
| ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| Store categories in `file_changes`                     | Frozen per diff, while settings hot-reload                                                                                   |
| Reorder the host's file list (rewrite `idx`)           | Desyncs store, MCP, blocks and flags keyed by `idx`; every re-partition would remap app state                                |
| Classify in the app only                               | The MCP server and CLI need the same verdicts                                                                                |
| `ignore`'s gitignore matcher                           | Last match wins; geld's per-category rescues need first-match with scoped negation                                           |
| `gix-glob`                                             | No brace alternatives                                                                                                        |
| No migration (stored bit OR patterns)                  | An explicit `-linguist-generated` would lose to the larger lists, and unticking lockfiles could not un-generate `Cargo.lock` |
| Change kinds as categories (geld's "trivial", "large") | Not path-based; large files already collapse behind "Load diff"                                                              |
