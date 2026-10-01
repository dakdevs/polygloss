# Contributing to Polygloss

Thanks for helping. Polygloss is a native macOS app in Rust (GPUI through gpui-kit) with a slim CLI and MCP server, plus TypeScript (Bun) test suites. This page covers setting up, the one cargo gotcha, the checks every change must pass, and the test rules. The design lives in [`docs/design.md`](docs/design.md) and [`docs/adr/`](docs/adr/README.md); the implementation plan, with its [Global constraints](docs/plan.md#global-constraints), in [`docs/plan.md`](docs/plan.md). Coding agents also follow [`AGENTS.md`](AGENTS.md).

## Setup

You need macOS 14 or later on Apple Silicon, and:

| Tool      | Version and notes                                                                                                 |
| --------- | ----------------------------------------------------------------------------------------------------------------- |
| git       | System git 2.39 or later (never bundled)                                                                          |
| Rust      | [rustup](https://rustup.rs); the pinned 1.98.1 toolchain installs itself from `rust-toolchain.toml`               |
| Dev tools | `cargo install cargo-nextest cargo-deny --locked` (into `~/.cargo/bin`); `cargo-insta` to review snapshot changes |
| Bun       | 1.3                                                                                                               |
| Packaging | Only for bundles: `cargo install cargo-packager --version =0.11.8 --locked`                                       |

No Metal toolchain is needed: shaders compile at runtime.

```bash
bun install --frozen-lockfile
scripts/cargo.sh build --workspace
target/debug/polygloss-cli --version     # polygloss 0.1.0
target/debug/Polygloss                   # the app, unbundled
```

## Cargo only through `scripts/cargo.sh`

Homebrew's `cargo`/`rustc` (1.93) sits earlier on `PATH` than rustup's on many Macs, and then doc-tests fail with `E0514`. **Never run bare `cargo`.** `scripts/cargo.sh` takes the same arguments and:

- runs rustup's cargo from `~/.cargo/bin` and points `RUSTC`/`RUSTDOC` at rustup's proxies;
- keeps build intermediates in `<main checkout>/target-shared`, shared by every git worktree, and each checkout's final binaries in `<checkout>/target` (so `target/debug/polygloss-cli` is always this checkout's build);
- serializes builds across worktrees with a lock. `cargo.sh: waiting for another cargo.sh using …` is a wait, not a hang. When another worktree used the shared dir last, it rebuilds the workspace crates (not the dependencies) first.

Never create other target dirs. `scripts/cargo.sh clean` wipes the shared cache when disk runs low; it only costs a cold rebuild. Details: [Cargo invocation](docs/plan.md#cargo-invocation-read-once).

## Checks

Run all of these before sending a change:

```bash
bun run format        # cargo fmt --all + prettier --write .
bun run lint          # clippy -D warnings + tsc --noEmit
bun run test:unit     # cargo nextest run --workspace
bun test              # bun suites (builds polygloss-cli, and the app for the docs checks)
bun run test:e2e      # GPUI E2E and screenshots, then the bun E2E suites against the running app
```

CI runs the same jobs on macOS arm64, plus the dependency audit:

```bash
bun run format:check
scripts/check-deps.sh                                          # crate-graph rules: slim CLI, no HTTP crates
scripts/cargo.sh deny check licenses bans sources advisories   # no GPL, AGPL or FSL; advisories
bun scripts/third-party-notices.ts --check                     # the bundled notices match Cargo.lock
```

When a dependency changes (`Cargo.lock`, or lumis's grammar features), regenerate `packaging/third-party-notices.md` with `bun scripts/third-party-notices.ts` and commit it; `package-release.sh` bundles it into `Contents/Resources` with the licenses and `NOTICE`. A new grammar that lumis vendors needs its upstream license added to the script first.

Other entry points:

| Command                                                                              | Does                                                                                                                                              |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `UPDATE_BASELINE=1 bun run test:e2e`                                                 | Re-records the screenshot baselines in `crates/polygloss-app/tests/baselines/`; open and check every changed PNG                                  |
| `bun run test:e2e tests/e2e/cli-app.test.ts`                                         | Only the named bun E2E suites (after the GPUI E2E run)                                                                                            |
| `POLYGLOSS_BUNDLE_E2E=1 bun run test:e2e`                                            | Also packages the release bundle (into `target/bundle-e2e.noindex`), runs the bundle suite, then the bun E2E suites against the bundle's binaries |
| `scripts/package-release.sh`                                                         | `dist/Polygloss.app` and the DMG, signed ad-hoc (`--sign` needs release credentials)                                                              |
| `scripts/fetch-sparkle.sh`                                                           | Downloads Sparkle 2.10.0 into `vendor/` (checksum-pinned), for release builds with updates                                                        |
| `POLYGLOSS_APPCAST_URL=<url> SPARKLE_PUBLIC_ED_KEY=<key> scripts/package-release.sh` | Also embeds Sparkle and its feed keys (both or neither; without them the app has no updater)                                                      |
| `scripts/make-appcast.sh dist`                                                       | The Sparkle appcast for the DMG, signed with `SPARKLE_PRIVATE_ED_KEY` (skips without it)                                                          |
| `scripts/smoke-bundle.sh dist/Polygloss.app`                                         | Launches the bundle in a sandbox and checks the socket, CLI link and URL scheme                                                                   |
| `bun benches/run-perf.ts --corpus typical --check-budgets --build`                   | Perf scenarios against the budgets in `benches/budgets.json` (needs an unlocked screen)                                                           |
| `bun scripts/git-parity.ts --repo <path> --range <a>..<b>`                           | Compares our hunks with `git diff` over a range of commits                                                                                        |
| `scripts/wake-gate/prepare.sh`                                                       | Sets up the manual agent wake-up gate ([docs/testing/agent-wake-gate.md](docs/testing/agent-wake-gate.md))                                        |

The user docs are tested too: `tests/scripts/docs.test.ts` holds [`docs/user-guide.md`](docs/user-guide.md) to the app's default key bindings and settings (`Polygloss --dump-keymap --json`, `--dump-settings --json`) and [`docs/agents.md`](docs/agents.md) to the MCP server's tools. When you add an action, a binding, a setting or a tool, update those docs in the same change.

## Test rules

- **Every change adds or updates tests.** Unit tests for logic, GPUI tests (`crates/polygloss-app/tests/app/`, one module per feature) for UI behavior, E2E tests (`tests/e2e/` in the app crate for the GPUI suites, `tests/e2e/` at the root for the agent surface) for features.
- **Never touch the real `HOME`.** Every test process gets its own temporary `HOME`, `POLYGLOSS_DATA_DIR`, `XDG_CONFIG_HOME`, a cache dir under that `HOME`, `GIT_CONFIG_GLOBAL` (an empty file) and `GIT_CONFIG_NOSYSTEM=1`. Use the helpers: `tests/support/sandbox.ts` for bun suites and `Sandbox` in `crates/polygloss-app/tests/support/` for Rust. Never read or write the real `~/Library` or `~/.config`, or Zed's or Claude's configuration.
- **Temporary repositories come from fixture scripts** (`scripts/make-fixture-repo.ts`, `tests/support/fixture-repo.ts`), with fixed identities and dates.
- The GPUI-linking crates have one integration test binary each (`tests/viewport/main.rs`, `tests/app/main.rs`) to keep link times down: add a module rather than a new test target.

## Code rules

- **Offline:** no network in the app or CLI (no HTTP crates), and never modify a user's index, HEAD or refs outside `refs/polygloss/`. Every git call goes through the core's runner. `tests/e2e/egress.test.ts` checks at runtime (`lsof -i`) that the app and `polygloss mcp` open no inet sockets.
- **Licenses:** everything is `MIT OR Apache-2.0`. Never add GPL, AGPL or FSL dependencies or code. Zed, GitComet, reviu, GitButler and pierre-native are study-only: never copy from them.
- **Crate boundaries:** the CLI links no GPUI, lumis or tree-sitter; `scripts/check-deps.sh` enforces the "must not contain" rules in [Workspace layout](docs/plan.md#workspace-layout-and-crate-ownership).
- **Naming:** kebab-case for files, crates, scripts and docs; snake_case only for Rust module files. TypeScript uses inline parameter types, never `interface`. `tests/scripts/repo-hygiene.test.ts` checks both.
- Commits: `<type>(<area>): <summary>`, for example `fix(viewport): keep the anchor on refresh`.

## License

By contributing you agree that your contributions are dual-licensed under `MIT OR Apache-2.0`, like the rest of the project.
