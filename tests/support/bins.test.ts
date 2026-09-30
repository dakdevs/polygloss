import { afterEach, describe, expect, test } from "bun:test";
import { join, resolve } from "node:path";
import { appBin, cliBin } from "./bins";

const repoRoot = resolve(import.meta.dir, "../..");
const saved = {
  POLYGLOSS_CLI_BIN: process.env.POLYGLOSS_CLI_BIN,
  POLYGLOSS_APP_BIN: process.env.POLYGLOSS_APP_BIN,
  CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR,
};

afterEach(() => {
  for (const [key, value] of Object.entries(saved)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
});

describe("test binaries", () => {
  test("cliBin defaults to this checkout's debug build", () => {
    delete process.env.POLYGLOSS_CLI_BIN;
    delete process.env.CARGO_TARGET_DIR;
    expect(cliBin()).toBe(join(repoRoot, "target", "debug", "polygloss-cli"));
  });

  test("cliBin honors POLYGLOSS_CLI_BIN", () => {
    process.env.POLYGLOSS_CLI_BIN = "/opt/release/polygloss-cli";
    expect(cliBin()).toBe("/opt/release/polygloss-cli");
  });

  test("a relative POLYGLOSS_CLI_BIN resolves against the repo root", () => {
    process.env.POLYGLOSS_CLI_BIN = "target/release/polygloss-cli";
    expect(cliBin()).toBe(join(repoRoot, "target", "release", "polygloss-cli"));
  });

  test("binaries follow a caller-provided CARGO_TARGET_DIR", () => {
    delete process.env.POLYGLOSS_CLI_BIN;
    delete process.env.POLYGLOSS_APP_BIN;
    process.env.CARGO_TARGET_DIR = "/tmp/elsewhere";
    expect(cliBin()).toBe("/tmp/elsewhere/debug/polygloss-cli");
    expect(appBin()).toBe("/tmp/elsewhere/debug/Polygloss");
  });

  test("appBin is the built Polygloss app", () => {
    delete process.env.CARGO_TARGET_DIR;
    delete process.env.POLYGLOSS_APP_BIN;
    expect(appBin()).toBe(join(repoRoot, "target", "debug", "Polygloss"));
  });

  test("appBin honors POLYGLOSS_APP_BIN (the bundle E2E run)", () => {
    process.env.POLYGLOSS_APP_BIN =
      "dist/Polygloss.app/Contents/MacOS/Polygloss";
    expect(appBin()).toBe(
      join(repoRoot, "dist", "Polygloss.app", "Contents", "MacOS", "Polygloss"),
    );
  });
});
