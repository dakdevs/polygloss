# Polygloss

A native macOS diff reviewer for code written with AI agents. Open a live worktree diff, a commit or a branch compare; review it GitHub-style with split or unified diffs, per-file Viewed, threaded draft comments and **Submit review**; and hand the review to Claude Code (or any MCP client) through a local MCP server and CLI that wake the agent on submit. Everything is local: no accounts, no network.

Built in Rust on [GPUI](https://www.gpui.rs) via gpui-kit, with our own diff viewport, system git and a local SQLite store.

## Status

**Pre-alpha.** The workspace is scaffolded and the implementation follows [`docs/plan.md`](docs/plan.md). Nothing is released yet.

## Requirements

| Tool      | Version                                                                                       |
| --------- | --------------------------------------------------------------------------------------------- |
| macOS     | 14 or later, Apple Silicon only                                                               |
| git       | system git 2.39 or later (never bundled)                                                      |
| Rust      | [rustup](https://rustup.rs); the pinned 1.98.1 toolchain installs from `rust-toolchain.toml`  |
| Dev tools | `cargo-nextest` and `cargo-deny` in `~/.cargo/bin` (`cargo-insta` to review snapshot changes) |
| Bun       | 1.3                                                                                           |

### The cargo `PATH` gotcha

A Homebrew `cargo`/`rustc` earlier on `PATH` shadows rustup and breaks doc-tests (`E0514`). Always run cargo through **`scripts/cargo.sh`**. It runs rustup's cargo from `~/.cargo/bin`, keeps build intermediates in `<main checkout>/target-shared` (shared by every git worktree) and final binaries in `<checkout>/target`, and serializes builds across worktrees. Details: [Cargo invocation](docs/plan.md#cargo-invocation-read-once).

## Build and test

Run from the repository root, in order:

```bash
bun install --frozen-lockfile
scripts/cargo.sh --version                        # cargo 1.98.1
scripts/cargo.sh build --workspace
target/debug/polygloss-cli --version              # polygloss 0.1.0
bun run format:check                              # cargo fmt --check + prettier --check
bun run lint                                      # clippy -D warnings + tsc --noEmit
bun run test:unit                                 # cargo nextest
bun test                                          # bun suites (builds polygloss-cli first)
bun run test:e2e                                  # GPUI E2E + screenshots, then MCP E2E
scripts/check-deps.sh                             # crate-graph rules (slim CLI, no GPL, offline)
scripts/cargo.sh deny check licenses bans sources
```

`bun run format` rewrites Rust and TypeScript formatting in place. Tests never touch your real `HOME`: every test process gets a temp `HOME`, data dir and git config.

## Layout

| Path                         | Contents                                                                            |
| ---------------------------- | ----------------------------------------------------------------------------------- |
| `crates/polygloss-diff`      | Diff model: hunks, word diff, line mapping, split/unified rows (no git, IO or GPUI) |
| `crates/polygloss-core`      | Git layer, ids, snapshots, SQLite store, events, review domain, IPC (no GPUI)       |
| `crates/polygloss-highlight` | lumis highlighting, compact tokens, themes (no GPUI)                                |
| `crates/polygloss-viewport`  | Our GPUI diff viewport                                                              |
| `crates/polygloss-platform`  | macOS glue: launching, Dock badge, Sparkle, editor detection, Install CLI (no GPUI) |
| `crates/polygloss-app`       | The `Polygloss` app                                                                 |
| `crates/polygloss-mcp`       | Agent API and the stdio MCP server                                                  |
| `crates/polygloss-cli`       | `polygloss-cli` (installed as `polygloss`): CLI, `mcp`, `wait`, JSON commands       |
| `crates/polygloss-perf`      | Perf harness (dev only)                                                             |
| `tests/`                     | bun suites                                                                          |
| `scripts/`                   | `cargo.sh`, `check-deps.sh` and other dev scripts                                   |
| `docs/`                      | Design, ADRs, research, plan                                                        |

## Docs

- [`docs/design.md`](docs/design.md): the design spec
- [`docs/adr/`](docs/adr/README.md): architecture decision records
- [`docs/plan.md`](docs/plan.md): the implementation plan
- [`docs/research/library-choices.md`](docs/research/library-choices.md): pinned libraries and why
- [`AGENTS.md`](AGENTS.md): rules for coding agents working in this repo

## License

Dual-licensed under `MIT OR Apache-2.0`, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE). See [NOTICE](NOTICE) for third-party credits.
