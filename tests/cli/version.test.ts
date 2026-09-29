import { afterAll, describe, expect, test } from "bun:test";
import { join, resolve } from "node:path";
import { cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

async function crateVersion(): Promise<string> {
  const manifest = Bun.TOML.parse(
    await Bun.file(join(repoRoot, "Cargo.toml")).text(),
  ) as { workspace: { package: { version: string } } };
  return manifest.workspace.package.version;
}

describe("polygloss-cli", () => {
  test("polygloss-cli --version prints the crate version", async () => {
    const r = Bun.spawnSync([cliBin(), "--version"], { env: sandbox.env });
    expect(r.stderr.toString()).toBe("");
    expect(r.exitCode).toBe(0);
    expect(r.stdout.toString()).toBe(`polygloss ${await crateVersion()}\n`);
  });
});
