// The license audit (plan T5.7, ADR-0004, design G7): the repo's deny.toml
// makes `cargo deny check licenses` fail on a copyleft dependency. Each case
// is a throwaway workspace (a root crate plus one path dependency with the
// license under test, so nothing is fetched) checked with the real deny.toml.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

setDefaultTimeout(60_000);

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

const env: Record<string, string> = {
  ...sandbox.env,
  // The real toolchain, read-only (cargo-deny lives in ~/.cargo/bin).
  CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), ".cargo"),
  RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"),
};

let count = 0;

/** A workspace whose root depends on a crate licensed `license`. */
function workspace(license: string): string {
  count += 1;
  const root = join(sandbox.home, `deny-${count}`);
  const dep = join(root, "dep");
  mkdirSync(join(root, "src"), { recursive: true });
  mkdirSync(join(dep, "src"), { recursive: true });
  writeFileSync(join(root, "src", "lib.rs"), "");
  writeFileSync(join(dep, "src", "lib.rs"), "");
  writeFileSync(
    join(root, "Cargo.toml"),
    [
      "[package]",
      'name = "deny-probe"',
      'version = "0.1.0"',
      'edition = "2024"',
      'license = "MIT OR Apache-2.0"',
      "publish = false",
      "",
      "[dependencies]",
      'probe-dep = { path = "dep" }',
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(dep, "Cargo.toml"),
    [
      "[package]",
      'name = "probe-dep"',
      'version = "0.1.0"',
      'edition = "2024"',
      `license = "${license}"`,
      "",
    ].join("\n"),
  );
  copyFileSync(join(repoRoot, "deny.toml"), join(root, "deny.toml"));
  return root;
}

function denyLicenses(root: string): { exitCode: number; output: string } {
  const r = Bun.spawnSync(
    [
      join(repoRoot, "scripts", "cargo.sh"),
      "deny",
      "--manifest-path",
      join(root, "Cargo.toml"),
      "--offline",
      // deny.toml is found next to the manifest.
      "check",
      "licenses",
    ],
    { env, cwd: root },
  );
  return {
    exitCode: r.exitCode ?? -1,
    output: r.stdout.toString() + r.stderr.toString(),
  };
}

describe("cargo deny check licenses", () => {
  test("deny rejects a GPL crate", () => {
    const r = denyLicenses(workspace("GPL-3.0-only"));
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("probe-dep");
    expect(r.output).toContain("GPL-3.0");
    expect(r.output).toContain("rejected");
  });

  for (const license of [
    "AGPL-3.0-or-later",
    "LGPL-2.1-only",
    "GPL-2.0-or-later OR GPL-3.0-only",
    "FSL-1.1-MIT",
  ])
    test(`deny rejects ${license}`, () => {
      const r = denyLicenses(workspace(license));
      expect(r.exitCode).not.toBe(0);
      expect(r.output).toContain("probe-dep");
      // A policy rejection, not a parse or config error.
      expect(r.output).toContain("rejected");
    });

  test("a permissive crate passes the same policy", () => {
    const r = denyLicenses(workspace("MIT OR Apache-2.0"));
    expect(r.output).toContain("licenses ok");
    expect(r.exitCode).toBe(0);
  });
});
