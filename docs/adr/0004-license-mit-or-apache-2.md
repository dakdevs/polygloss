# ADR-0004: License `MIT OR Apache-2.0`

- **Status:** Accepted. GPL was dropped along with the Zed plan.
- **Date:** 2026-09-28
- **Design:** [§21 Packaging and distribution](../design.md#21-packaging-and-distribution)

## Context

Polygloss will be open source. GPL-3.0 was considered only while the plan was to fork Zed. The chosen dependencies are all permissive:

- gpui-kit (Apache-2.0)
- gix (MIT/Apache)
- rusqlite (MIT)
- rmcp (Apache-2.0)
- lumis (MIT)
- nucleo-matcher (MPL-2.0, used unmodified)
- Lilex font (OFL)
- `@pierre/theme` (Apache-2.0)

## Decision

Dual-license Polygloss as **`MIT OR Apache-2.0`**. Use the **system git** and do not bundle it, so no GPL code ships. Never copy GPL, AGPL or FSL code: Zed, GitComet, reviu and GitButler are study-only.

## Consequences

- `cargo-deny` in CI rejects GPL, AGPL and FSL dependencies.
- `NOTICE` credits the Pierre theme port (Apache-2.0) and Lilex (OFL).
- We write our own versions of things Zed already has: the CLI install command, CLI-to-app IPC and the screenshot baseline runner.
- The MPL-2.0 crate stays unmodified.

## Alternatives rejected

| Option               | Why not                                                             |
| -------------------- | ------------------------------------------------------------------- |
| GPL-3.0-or-later     | Only needed to use Zed crates, and that plan was dropped (ADR-0003) |
| Proprietary          | The user wants open source                                          |
| Bundling a git build | Would ship GPL-2 code and require a source offer                    |
