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

/** The debug `Polygloss` app (E2E only; built by scripts/test-e2e.sh). */
export function appBin(): string {
  return resolve(targetDir(), "debug", "Polygloss");
}
