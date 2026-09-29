# AGENTS.md

Rules for coding agents in this repo. The full list is [Global constraints](docs/plan.md#global-constraints) in the plan; read it and your task card before changing anything. Spec: [`docs/design.md`](docs/design.md), [`docs/adr/`](docs/adr/README.md).

## Non-negotiable

- **Cargo only via `scripts/cargo.sh`.** Never bare `cargo`: Homebrew's 1.93 shadows rustup on `PATH`. Never create other target dirs.
- **Never touch the real `HOME` in tests.** Every test process gets its own temp `HOME`, `POLYGLOSS_DATA_DIR`, `XDG_CONFIG_HOME`, `GIT_CONFIG_GLOBAL` (empty file) and `GIT_CONFIG_NOSYSTEM=1`. Never read or write the real `~/Library` or `~/.config`, or Zed's or Claude's config.
- **Never add GPL, AGPL or FSL dependencies or code.** License is `MIT OR Apache-2.0`. Zed, GitComet, reviu, GitButler and pierre-native are study-only.
- **No network** in the app or CLI, and never modify the user's index, HEAD or refs outside `refs/polygloss/`.
- Naming: kebab-case everywhere except snake_case Rust module files (ADR-0018). TypeScript uses inline parameter types, never `interface`. `tests/scripts/repo-hygiene.test.ts` enforces both.
- Every change adds or updates tests.

## Completion block

Run all of these before calling a task done:

```bash
bun run format            # cargo fmt --all + prettier --write .
bun run lint              # clippy -D warnings + tsc --noEmit
bun run test:unit         # cargo nextest run --workspace
bun test                  # bun suites
bun run test:e2e          # build + GPUI E2E/screenshots + MCP E2E
```
