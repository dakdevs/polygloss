// bun test preload (bunfig.toml): build polygloss-cli once per `bun test` run so
// every suite exercises this checkout's binary. POLYGLOSS_SKIP_BUILD=1 skips it
// (scripts/test-e2e.sh builds first; POLYGLOSS_CLI_BIN users bring their own).
import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");

if (process.env.POLYGLOSS_SKIP_BUILD !== "1") {
  const argv = ["scripts/cargo.sh", "build", "-p", "polygloss-cli"];
  // cargo's stderr streams live (progress, "waiting for another cargo.sh …").
  const r = Bun.spawnSync(argv, {
    cwd: repoRoot,
    env: process.env,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (r.exitCode !== 0) {
    throw new Error(
      `tests/support/preload.ts: \`${argv.join(" ")}\` failed with exit code ${r.exitCode}; cargo's errors are above`,
    );
  }
}
