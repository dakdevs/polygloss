import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "./sandbox";

const preload = resolve(import.meta.dir, "preload.ts");
const sandbox = makeSandbox();
let cargoHome = "";
let log = "";

// A fake rustup cargo for scripts/cargo.sh: logs its argv and fails when asked.
const fakeCargo = `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_CARGO_LOG"
if [ -n "\${FAKE_CARGO_FAIL-}" ]; then
  echo "error: could not compile \\\`polygloss-cli\\\` (fake)" >&2
  exit 101
fi
`;

beforeAll(() => {
  cargoHome = join(sandbox.home, "cargo-home");
  mkdirSync(join(cargoHome, "bin"), { recursive: true });
  writeFileSync(join(cargoHome, "bin", "cargo"), fakeCargo);
  chmodSync(join(cargoHome, "bin", "cargo"), 0o755);
  log = join(sandbox.home, "cargo.log");
});

afterAll(() => sandbox.cleanup());

function runPreload(env: Record<string, string>): {
  exitCode: number;
  stderr: string;
} {
  const r = Bun.spawnSync([process.execPath, preload], {
    env: {
      ...sandbox.env,
      CARGO_HOME: cargoHome,
      CARGO_TARGET_DIR: join(sandbox.home, "target"),
      CARGO_BUILD_BUILD_DIR: join(sandbox.home, "target-shared"),
      FAKE_CARGO_LOG: log,
      ...env,
    },
  });
  return { exitCode: r.exitCode ?? -1, stderr: r.stderr.toString() };
}

function logLines(): string[] {
  return existsSync(log)
    ? readFileSync(log, "utf8").split("\n").filter(Boolean)
    : [];
}

describe("tests/support/preload.ts", () => {
  test("builds polygloss-cli through scripts/cargo.sh", () => {
    writeFileSync(log, "");
    const r = runPreload({});
    expect(r.exitCode).toBe(0);
    expect(logLines()).toEqual(["build -p polygloss-cli"]);
  });

  test("fails loudly with cargo's stderr when the build fails", () => {
    writeFileSync(log, "");
    const r = runPreload({ FAKE_CARGO_FAIL: "1" });
    expect(r.exitCode).not.toBe(0);
    expect(r.stderr).toContain("could not compile `polygloss-cli` (fake)");
    expect(r.stderr).toContain("scripts/cargo.sh build -p polygloss-cli");
  });

  test("POLYGLOSS_SKIP_BUILD=1 skips the build", () => {
    writeFileSync(log, "");
    const r = runPreload({ POLYGLOSS_SKIP_BUILD: "1" });
    expect(r.exitCode).toBe(0);
    expect(logLines()).toEqual([]);
  });
});
