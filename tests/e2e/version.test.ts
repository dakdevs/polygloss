import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  type AppWorld,
  appCall,
  appWorld,
  startApp,
  stopAllApps,
} from "../support/app";
import { appBin, builtVersion, cliBin } from "../support/bins";

// A release's POLYGLOSS_VERSION when the build had one (CI's End-to-end job
// builds with one), else the crate version.
const built = builtVersion();

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
