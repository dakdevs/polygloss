# ADR-0002: gpui-kit as the UI foundation

- **Status:** Accepted. Supersedes the Zed-spike plan to drop gpui-kit.
- **Date:** 2026-09-28
- **Design:** [§11 UI](../design.md#11-ui), [§23 Libraries](../design.md#23-libraries)

## Context

The `gpui` crate on crates.io has been stuck at 0.2.2 since 2025-10. The live sources are Zed git revisions and Longbridge's weekly `gpui-pre` snapshots. Polygloss needs a lot of chrome: a tree, virtual lists, a textarea, markdown, a command palette, popovers, notifications, resizable panes, native menus and test locators. The Zed-fork plan would have swapped gpui-kit for Zed's GPL `ui` crate, but the Zed fork was rejected (ADR-0003).

## Decision

Use **gpui-kit 0.7** (formerly gpui-component, Apache-2.0), pinned exactly on **gpui-pre `=0.3.7`**, as the only GPUI source. It supplies Tree, VirtualList, Textarea, `TextView::markdown`, Command, Popover, Notification, resizable panes, `native_menu` and test locators. The diff viewport is our own GPUI element. We accept regular upgrade work, since releases break weekly.

## Consequences

- Exactly one GPUI source. A Zed git rev cannot be mixed in; two GPUI runtimes will not unify.
- Upgrades are scheduled work: one PR per bump, gated by screenshot tests. gpui-kit 0.6 to 0.7 took 25 days.
- Known gaps we fill ourselves:
  - A stop-propagation wrapper around the Viewed checkbox in Tree rows.
  - Directory compaction done in advance.
  - `nucleo-matcher` ranking, because the built-in filter is substring-only.
  - A markdown sanitizer, because `TextView` renders HTML and images.
- Requires rustc ≥ 1.95. 1.98.1 is verified.

## Alternatives rejected

| Option                      | Why not                                                      |
| --------------------------- | ------------------------------------------------------------ |
| Bare Zed git rev + Zed `ui` | The `ui` crate is GPL, and we would be tied to Zed internals |
| Hand-rolled widgets         | Breaks the "best existing library first" rule                |
| gpui-ce                     | Releases were yanked, and it is diverging                    |
