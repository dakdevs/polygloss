import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  type AppWorld,
  appCall,
  appWorld,
  startApp,
  stopAllApps,
} from "../support/app";
import { appBin, cliBin } from "../support/bins";

// The version the executables were built with (ADR-0019): a release's
// POLYGLOSS_VERSION when the build had one (CI's End-to-end job builds with
// one), else the crate version. scripts/test-e2e.sh builds with this
// environment, so the two agree.
const repoRoot = resolve(import.meta.dir, "../..");
const crateVersion = (
  Bun.TOML.parse(readFileSync(join(repoRoot, "Cargo.toml"), "utf8")) as {
    workspace: { package: { version: string } };
  }
).workspace.package.version;
const built = process.env.POLYGLOSS_VERSION || crateVersion;

describe.skipIf(!process.env.POLYGLOSS_E2E)("built version", () => {
  // Made in beforeAll: a skipped describe runs its body but no hooks.
  let world: AppWorld;
  beforeAll(() => {
    world = appWorld();
  });
  afterAll(async () => {
    await stopAllApps(world.socket);
    world.cleanup();
  });

  test(`Polygloss, polygloss-cli and the running app report ${built}`, async () => {
    for (const [bin, name] of [
      [appBin(), "Polygloss"],
      [cliBin(), "polygloss"],
    ] as const) {
      const r = Bun.spawnSync([bin, "--version"], { env: world.env });
      expect(r.exitCode).toBe(0);
      expect(r.stdout.toString()).toBe(`${name} ${built}\n`);
    }
    await startApp(world);
    const hello = await appCall(world.socket, "hello", { client: "e2e-tests" });
    expect(hello.version).toBe(built);
  }, 90_000);
});
