# ADR-0017: Tests split between cargo and bun

- **Status:** Accepted. Supersedes the note about using Zed's GPL test-support.
- **Date:** 2026-09-28
- **Design:** [§20 Testing strategy](../design.md#20-testing-strategy)

## Context

The app is written in Rust, but the agent surface (MCP and the JSON CLI) is best tested the way agents use it: as a separate process, driven by the official MCP client. Project rules say every change adds tests, and that `bun test` and `bun run test:e2e` must pass. While the plan was to fork Zed, its GPL test-support was an option. Under ADR-0004 it is not.

## Decision

- **Rust:** `cargo nextest` and `insta` snapshots, `#[gpui_kit::test]` (GPUI's test macro via gpui-kit) with `TestAppContext` / `VisualTestContext`, and gpui-kit `test-support` locators. We write our own screenshot baseline runner.
- **Agent surface:** `bun test` TypeScript suites spawn `polygloss mcp` through `@modelcontextprotocol/client` against temp git repos and a temp database, and exercise the JSON CLI.
- Root `package.json` scripts:
  - `test:unit` runs nextest.
  - `test:e2e` builds, then runs GPUI E2E with screenshots, then MCP E2E.
  - `lint` runs `clippy -D warnings` and `tsc`.
  - `format` runs `cargo fmt` and prettier.
- CI runs on macOS arm64.

## Consequences

- Two toolchains (cargo and bun) are required for development.
- Every test sandboxes `HOME`, the data directory and the git config. A spike once damaged the user's real Zed log, and that must not happen again.
- Git parity and performance corpora run as scripts, performance nightly.
- The wake path still needs a manual gate in real Claude Code.

## Alternatives rejected

| Option                                  | Why not                                                                            |
| --------------------------------------- | ---------------------------------------------------------------------------------- |
| Rust-only tests, MCP included           | Doesn't test the real client or process boundary; project rules require `bun test` |
| MCP Inspector CLI as the main harness   | Hard to script assertions with; kept as a manual tool                              |
| Zed `visual_test_runner` / test-support | GPL                                                                                |
