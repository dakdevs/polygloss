import { describe, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { appBin } from "../support/bins";

// The cheapest E2E check: scripts/test-e2e.sh built the app. Its own test
// (tests/scripts/test-e2e.test.ts) runs only this suite against a fake build;
// the other suites here drive the real app. scripts/test-e2e.sh sets POLYGLOSS_E2E.
describe.skipIf(!process.env.POLYGLOSS_E2E)("app E2E smoke", () => {
  test("appBin() points at the built Polygloss app", () => {
    expect(existsSync(appBin())).toBe(true);
  });
});
