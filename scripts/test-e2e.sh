#!/usr/bin/env bash
# E2E suites: GPUI E2E/screenshot tests (polygloss-app's `e2e` test binary,
# feature `e2e`), then the bun E2E suites against the built binaries.
# Until M2 adds tests/e2e/main.rs, the nextest step finds no tests and passes.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

# One -p per build: a joint build would unify features and link
# polygloss-platform's `appkit` into the CLI (see plan T5.1).
scripts/cargo.sh build -p polygloss-app
scripts/cargo.sh build -p polygloss-cli
scripts/cargo.sh nextest run -p polygloss-app --features e2e --no-tests=warn -E 'binary(e2e)'
POLYGLOSS_E2E=1 POLYGLOSS_SKIP_BUILD=1 bun test tests/e2e
