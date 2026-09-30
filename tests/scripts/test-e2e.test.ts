import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
let cargoHome = "";
let targetDir = "";
let log = "";

// A fake rustup cargo: logs argv, "builds" Polygloss, fails on FAKE_CARGO_FAIL.
const fakeCargo = `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_CARGO_LOG"
case "$*" in *"$FAKE_CARGO_FAIL"*) echo "fake failure: $*" >&2; exit 101 ;; esac
if [ "$1" = build ] && [ -n "\${FAKE_CARGO_MAKE_APP-}" ]; then
  mkdir -p "$CARGO_TARGET_DIR/debug" && : >"$CARGO_TARGET_DIR/debug/Polygloss"
fi
`;

beforeAll(() => {
  cargoHome = join(sandbox.home, "cargo-home");
  mkdirSync(join(cargoHome, "bin"), { recursive: true });
  writeFileSync(join(cargoHome, "bin", "cargo"), fakeCargo);
  chmodSync(join(cargoHome, "bin", "cargo"), 0o755);
});

afterAll(() => sandbox.cleanup());

// The fake build makes an empty `Polygloss` and no CLI, so only the smoke
// suite can run against it (the other suites drive the real app).
const SMOKE = "tests/e2e/smoke.test.ts";

function runE2e(
  env: Record<string, string>,
  args: string[] = [SMOKE],
): {
  exitCode: number;
  output: string;
  log: string[];
} {
  targetDir = join(
    sandbox.home,
    `target-${Math.random().toString(36).slice(2)}`,
  );
  log = join(sandbox.home, "cargo.log");
  rmSync(log, { force: true });
  const r = Bun.spawnSync([join(repoRoot, "scripts", "test-e2e.sh"), ...args], {
    cwd: sandbox.home,
    env: {
      ...sandbox.env,
      CARGO_HOME: cargoHome,
      CARGO_TARGET_DIR: targetDir,
      CARGO_BUILD_BUILD_DIR: join(sandbox.home, "target-shared"),
      FAKE_CARGO_LOG: log,
      FAKE_CARGO_FAIL: "<no-such-step>",
      ...env,
    },
  });
  return {
    exitCode: r.exitCode ?? -1,
    output: r.stdout.toString() + r.stderr.toString(),
    log: existsSync(log)
      ? readFileSync(log, "utf8").split("\n").filter(Boolean)
      : [],
  };
}

describe("scripts/test-e2e.sh", () => {
  test("builds each binary on its own, runs the e2e nextest binary, then the bun E2E suite", () => {
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1" });
    expect(r.output).toContain("app E2E smoke");
    expect(r.exitCode).toBe(0);
    expect(r.log).toEqual([
      "build -p polygloss-app",
      "build -p polygloss-cli",
      "nextest run -p polygloss-app --features e2e --no-tests=warn -E binary(e2e)",
    ]);
  });

  test("the bun E2E suite runs with POLYGLOSS_E2E set and fails without the app", () => {
    const r = runE2e({});
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("appBin() points at the built Polygloss app");
    expect(r.log.length).toBe(3);
  });

  test("without arguments the bun step runs every suite in tests/e2e", () => {
    // Against the fake (empty, non-executable) app every app suite fails
    // fast, but each one runs.
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1" }, []);
    expect(r.exitCode).not.toBe(0);
    for (const suite of [
      "smoke.test.ts",
      "mcp-app.test.ts",
      "cli-app.test.ts",
      "mcp-multi-process-writers.test.ts",
    ])
      expect(r.output).toContain(`tests/e2e/${suite}`);
  });

  test("with POLYGLOSS_BUNDLE_E2E=1 it then packages the release bundle", () => {
    // The fake packager makes no bundle, so package-release.sh stops there;
    // the bundle suite itself is tests/scripts/package.test.ts.
    const dist = join(sandbox.home, "dist-bundle-e2e");
    const r = runE2e({
      FAKE_CARGO_MAKE_APP: "1",
      POLYGLOSS_BUNDLE_E2E: "1",
      POLYGLOSS_DIST_DIR: dist,
    });
    expect(r.output).toContain("app E2E smoke");
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("cargo packager made no");
    expect(r.log.slice(3)).toEqual([
      "build --release -p polygloss-app",
      "build --release -p polygloss-cli",
      `packager --release --formats app --out-dir ${dist}`,
    ]);
  });

  test("stops at the first failing step", () => {
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1", FAKE_CARGO_FAIL: "nextest" });
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("fake failure: nextest");
    expect(r.output).not.toContain("app E2E smoke");
  });
});
