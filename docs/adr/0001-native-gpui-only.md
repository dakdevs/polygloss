# ADR-0001: Native GPUI only

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§1 Overview](../design.md#1-overview), [§23 Libraries](../design.md#23-libraries)

## Context

Polygloss is a desktop diff viewer that has to stay fast on huge diffs. Research compared four stacks:

- pure Rust on GPUI
- GPUiX (TypeScript/React on GPUI) with erwinkn/pierre-native
- a web shell (Electron or Tauri) with `@pierre/diffs`
- a TypeScript-core hybrid

On delivery risk alone, research recommended web-first, because `@pierre/diffs` already exists. The GPUiX path depends on a fork of a fork. pierre-native has 3 commits, 0 stars and no releases, and research measured a 17 s stall with 366 MB of JSON at 500 files.

## Decision

Build Polygloss in **pure Rust on GPUI, native only**. There are no web views anywhere, including for comment markdown. The user explicitly rejected Tauri, embedded web views (gpui-wry) and, by extension, every web shell.

## Consequences

- We own the diff viewport (ADR-0003). No permissive GPUI diff viewer exists.
- Performance has the highest ceiling, but the viewport's performance has not been measured yet (design §12).
- GPUI releases break things often, so upgrades need scheduled work (ADR-0002).
- UI tests run in Rust. The agent surface is tested black-box from bun (ADR-0017).

## Alternatives rejected

| Option                           | Why not                                                                                          |
| -------------------------------- | ------------------------------------------------------------------------------------------------ |
| Electron + `@pierre/diffs`       | Not native, and carries a Chromium footprint. It was research's top pick; the user overruled it. |
| Tauri 2 + `@pierre/diffs`        | Not native. The WKWebView worker bug (tauri#9975) was never ruled out.                           |
| GPUiX + pierre-native            | Fork of a fork, unreleased, and measured perf blockers.                                          |
| gpui-wry hosting `@pierre/diffs` | An embedded web view inside a native app. Rejected by the user.                                  |
