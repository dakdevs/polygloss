#!/usr/bin/env bash
# E2E suites: GPUI E2E/screenshot tests (polygloss-app's `e2e` test binary,
# feature `e2e`), then the bun E2E suites against the built binaries (the
# agent surface against a running app, plan T4.11). With
# POLYGLOSS_BUNDLE_E2E=1 it then packages the release bundle
# (scripts/package-release.sh) and runs the bundle suite against it (plan
# T5.1: static checks, scripts/smoke-bundle.sh, the URL scheme, the DMG).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

# One -p per build: a joint build would unify features and link
# polygloss-platform's `appkit` into the CLI (see plan T5.1).
scripts/cargo.sh build -p polygloss-app
scripts/cargo.sh build -p polygloss-cli
scripts/cargo.sh nextest run -p polygloss-app --features e2e --no-tests=warn -E 'binary(e2e)'
# Arguments replace the bun suites' path filter (default: all of tests/e2e),
# e.g. `bun run test:e2e tests/e2e/cli-app.test.ts`.
if [ "$#" -eq 0 ]; then set -- tests/e2e; fi
POLYGLOSS_E2E=1 POLYGLOSS_SKIP_BUILD=1 bun test "$@"
if [ "${POLYGLOSS_BUNDLE_E2E-}" = 1 ]; then
  scripts/package-release.sh
  POLYGLOSS_BUNDLE="${POLYGLOSS_DIST_DIR:-$repo_root/dist}/Polygloss.app" \
    POLYGLOSS_SKIP_BUILD=1 bun test tests/scripts/package.test.ts
fi
