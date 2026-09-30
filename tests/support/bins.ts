// Paths of this checkout's built binaries (scripts/cargo.sh puts final
// artifacts in <checkout>/target unless the caller set CARGO_TARGET_DIR).
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");

function targetDir(): string {
  return resolve(repoRoot, process.env.CARGO_TARGET_DIR || "target");
}

/** The `polygloss-cli` under test: `$POLYGLOSS_CLI_BIN`, else the debug build. */
export function cliBin(): string {
  const override = process.env.POLYGLOSS_CLI_BIN;
  if (override) return resolve(repoRoot, override);
  return resolve(targetDir(), "debug", "polygloss-cli");
}

/**
 * The `Polygloss` app under test (E2E only): `$POLYGLOSS_APP_BIN` when set
 * (scripts/test-e2e.sh points it at the release bundle's executable with
 * POLYGLOSS_BUNDLE_E2E=1), else the debug build scripts/test-e2e.sh made.
 */
export function appBin(): string {
  const override = process.env.POLYGLOSS_APP_BIN;
  if (override) return resolve(repoRoot, override);
  return resolve(targetDir(), "debug", "Polygloss");
}
