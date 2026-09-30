# Polygloss

A native macOS diff reviewer for code written with AI agents. Open a live worktree diff, a commit or a branch compare; review it GitHub-style with split or unified diffs, per-file Viewed, threaded draft comments and **Submit review**; and hand the review to Claude Code (or any MCP client) through a local MCP server and CLI that wake the agent on submit. Everything is local: no accounts, no network.

Built in Rust on [GPUI](https://www.gpui.rs) via gpui-kit, with our own diff viewport, system git and a local SQLite store.

## Status

**Pre-release.** v1 is feature-complete and in release testing; there is no public release yet. Until then, [build it from source](#build-from-source).

## Install

Polygloss needs macOS 14 or later on Apple Silicon, and git 2.39 or later (the system git from the Xcode command line tools is fine).

**Homebrew** (installs the app and puts the `polygloss` CLI on your `PATH`):

```bash
brew install --cask dakdevs/tap/polygloss
```

**DMG:** download `Polygloss_<version>_aarch64.dmg` from [GitHub Releases](https://github.com/dakdevs/polygloss/releases), drag Polygloss to Applications and launch it once. For the CLI, run **Install CLI** from the command palette (`⌘K`) or the Polygloss menu; it links `/usr/local/bin/polygloss` to the app's CLI, asking for your password if needed. Polygloss can check for updates (it asks on first launch; the check is its only network access).

### Claude Code plugin

The plugin adds Polygloss's MCP server, a hook that wakes Claude when you submit a review, and a skill that teaches Claude the review loop:

```bash
claude plugin marketplace add dakdevs/polygloss
claude plugin install polygloss@polygloss
```

Other agents (Codex, Cursor, Claude Desktop, …) run `polygloss mcp` as a stdio MCP server or use the JSON CLI: see [docs/agents.md](docs/agents.md#other-mcp-clients).

## Quick start

```bash
cd ~/code/my-project
polygloss                       # review your working tree against its merge-base with the default branch
polygloss show HEAD             # one commit
polygloss compare main feature  # a branch, like a pull request (three-dot)
```

In the app: `j`/`k` move through lines, `n`/`p` through files, `v` marks a file viewed, `c` comments (`⌘⏎` saves the draft), and `⇧⌘⏎` submits the review with a verdict. `⌘K` lists every action and `?` every key. `⌘O` opens anything from the app.

With the plugin, ask Claude to "open this in Polygloss for review". It opens its changes in the app and ends its turn; when you press **Submit review**, it wakes up, reads your threads, fixes the code, replies, and asks for a re-review.

## Docs

- [User guide](docs/user-guide.md): reviewing, comments and Submit review, Viewed, live mode, keys, settings, themes
- [Agents](docs/agents.md): the Claude Code plugin, MCP tools, the JSON CLI, sessions and wake-up, other MCP clients
- [Contributing](CONTRIBUTING.md): setup, the cargo gotcha, checks and test rules
- [Design](docs/design.md), [ADRs](docs/adr/README.md), [implementation plan](docs/plan.md), [library choices](docs/research/library-choices.md)
- [Agent wake-up gate](docs/testing/agent-wake-gate.md): the manual release check in real Claude Code
- [`AGENTS.md`](AGENTS.md): rules for coding agents working in this repo

## Build from source

You need [rustup](https://rustup.rs) (the pinned toolchain installs itself), `cargo-nextest` and `cargo-deny` in `~/.cargo/bin`, and Bun 1.3. Always run cargo through `scripts/cargo.sh`: a Homebrew `cargo` earlier on `PATH` shadows rustup's and breaks the build ([details](CONTRIBUTING.md#cargo-only-through-scriptscargosh)).

```bash
bun install --frozen-lockfile
scripts/cargo.sh build --workspace
target/debug/Polygloss                  # the app
target/debug/polygloss-cli --version    # the CLI (installed as `polygloss`)
scripts/package-release.sh              # dist/Polygloss.app and the DMG (needs cargo-packager 0.11.8)
```

Checks, as CI runs them:

```bash
bun run format:check    # cargo fmt --check + prettier --check
bun run lint            # clippy -D warnings + tsc --noEmit
bun run test:unit       # cargo nextest
bun test                # bun suites
bun run test:e2e        # GPUI E2E and screenshots, then the agent-surface E2E
scripts/check-deps.sh   # crate-graph rules (slim CLI, no GPL, offline)
```

Tests never touch your real `HOME`: every test process gets a temporary `HOME`, data dir and git config.

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
| `plugins/polygloss`          | The Claude Code plugin (marketplace in `.claude-plugin/`)                           |
| `tests/`                     | bun suites                                                                          |
| `scripts/`                   | `cargo.sh`, `check-deps.sh`, packaging and other dev scripts                        |
| `docs/`                      | User guide, agents, design, ADRs, research, plan, manual test procedures            |

## License

Dual-licensed under `MIT OR Apache-2.0`, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE). See [NOTICE](NOTICE) for third-party credits.
