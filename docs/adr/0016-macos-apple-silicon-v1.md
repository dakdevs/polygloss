# ADR-0016: v1 targets macOS on Apple Silicon

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§22 Platform scope](../design.md#22-platform-scope)

## Context

GPUI is most mature on macOS. The user's machines and CI runners are Apple Silicon. Supporting more platforms multiplies packaging, testing and platform-specific code (URL schemes, notifications, launching, updates).

## Decision

- **v1 supports macOS on Apple Silicon (arm64) only.**
- Keep the code portable:
  - `notify` for file watching
  - `interprocess` for sockets
  - one `polygloss-platform` crate for every macOS-specific call: launching, Sparkle, the Dock badge, editor detection and Install CLI. It links no GPUI, so the slim CLI can use its launcher.
  - notifications and URL opening through GPUI's portable APIs (`show_system_notification`, `on_open_urls`); the URL-scheme registration is Info.plist packaging
- Linux comes next, Windows later.

## Consequences

- CI runs only on macOS arm64. Minimum macOS version is Provisional: 14 (OQ-18).
- No universal binary and no Intel testing.
- GPUI's `register_url_scheme` is not implemented on Windows. That gets handled when Windows is in scope.

## Alternatives rejected

| Option                       | Why not                                            |
| ---------------------------- | -------------------------------------------------- |
| Universal binary (Intel too) | Extra CI and testing cost for a shrinking platform |
| Cross-platform from day one  | Platform gaps and too much v1 scope                |
