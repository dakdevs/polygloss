// bun test preload (bunfig.toml): build polygloss-cli once per `bun test` run so
// every suite exercises this checkout's binary. POLYGLOSS_SKIP_BUILD=1 skips it
// (scripts/test-e2e.sh builds first; POLYGLOSS_CLI_BIN users bring their own).
//
// Then every later Bun.spawnSync that sets no `timeout` gets one,
// POLYGLOSS_TEST_SPAWN_TIMEOUT_MS (default 5 minutes). Bun 1.3.14's per-test
// timeout does not interrupt a spawnSync whose process tree hangs (seen with a
// hung tool three processes below scripts/test-e2e.sh): the test, and the run,
// wait for it, as CI's bun job once did for its full 60 minutes. spawnSync's
// own `timeout` does fire: the child gets SIGTERM, spawnSync returns, and the
// test fails under its own name.
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

const timeout = Number(
  process.env.POLYGLOSS_TEST_SPAWN_TIMEOUT_MS ?? 5 * 60_000,
);
Bun.spawnSync = new Proxy(Bun.spawnSync, {
  apply: (spawnSync, thisArg, [first, options]) =>
    Reflect.apply(
      spawnSync,
      thisArg,
      Array.isArray(first)
        ? [first, { timeout, ...options }]
        : [{ timeout, ...first }],
    ),
});
