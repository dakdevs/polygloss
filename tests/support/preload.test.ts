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

  test("a Bun.spawnSync without a timeout fails its test when the child hangs", () => {
    // Each child waits 30 s on a grandchild that holds its pipes; uncapped,
    // the run would take a minute.
    const suite = join(sandbox.home, "hang.test.ts");
    writeFileSync(
      suite,
      `import { expect, test } from "bun:test";
const hang = ["bash", "-c", "sleep 30; echo done"];
test("hangs in argv form", () => {
  expect(Bun.spawnSync(hang).exitCode).toBe(0);
}, 120_000);
test("hangs in options form", () => {
  expect(Bun.spawnSync({ cmd: hang }).exitCode).toBe(0);
}, 120_000);
`,
    );
    const started = Date.now();
    const r = Bun.spawnSync(
      [process.execPath, "test", "--preload", preload, suite],
      {
        cwd: sandbox.home,
        env: {
          ...sandbox.env,
          POLYGLOSS_SKIP_BUILD: "1",
          POLYGLOSS_TEST_SPAWN_TIMEOUT_MS: "1000",
        },
        timeout: 50_000,
      },
    );
    const output = r.stdout.toString() + r.stderr.toString();
    expect(Date.now() - started).toBeLessThan(20_000);
    expect(r.exitCode).toBe(1);
    expect(output).toContain("(fail) hangs in argv form");
    expect(output).toContain("(fail) hangs in options form");
  }, 60_000);
});
