# ADR-0025: Keyboard-first, remappable keymap

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§11.9 Keymap](../design.md#119-keymap-adr-0025), [§18 Settings and keymap files](../design.md#18-settings-and-keymap-files)

## Context

Reviewing hundreds of files needs fast navigation without the mouse. GPUI has first-class actions, key contexts and multi-stroke bindings, and gpui-kit's Command palette shows each binding as a hint.

## Decision

Every command is a GPUI action. Bindings can be remapped in `~/.config/polygloss/keymap.json`, which reloads on change. Defaults:

| Keys                                              | Action                                              |
| ------------------------------------------------- | --------------------------------------------------- |
| `j` `k` / arrows                                  | Move the line cursor                                |
| `n` `p`                                           | Next / previous file                                |
| `]` `[`                                           | Next / previous change                              |
| `v`                                               | Viewed, then collapse and jump to the next unviewed |
| `c` (`shift+arrows` for a range), `⌘⏎` save draft | Comment                                             |
| `.` `,`                                           | Next / previous open thread                         |
| `e` / `E`                                         | Expand context / expand whole file                  |
| `s`                                               | Split ↔ unified                                     |
| `w`                                               | Hide whitespace                                     |
| `R`                                               | Refresh                                             |
| `o`                                               | Open in editor                                      |
| `⌘P`                                              | File finder                                         |
| `⌘K`                                              | Command palette                                     |
| `⌘⇧⏎`                                             | Submit review                                       |
| `?`                                               | Cheat sheet                                         |

A Vim mode may come later as an option. It would be our own implementation; Zed's Vim mode is GPL and off-limits.

## Consequences

- One action registry feeds the palette, menus, the cheat sheet and keymap overrides.
- Focus scoping between the tree, viewport and composer uses key contexts. Single-letter keys are inactive while typing in the composer.
- A few extra macOS-standard bindings are Provisional (design §11.9).

## Alternatives rejected

| Option                  | Why not                                      |
| ----------------------- | -------------------------------------------- |
| Hard-coded shortcuts    | Users can't adapt them                       |
| Vim bindings by default | Unfamiliar to most reviewers; optional later |
| Mouse-first UI          | Too slow for reviewing large diffs           |
