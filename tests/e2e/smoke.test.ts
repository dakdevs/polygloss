import { describe, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { appBin } from "../support/bins";

// Placeholder until M2 adds real app E2E suites; scripts/test-e2e.sh sets POLYGLOSS_E2E.
describe.skipIf(!process.env.POLYGLOSS_E2E)("app E2E smoke", () => {
  test("appBin() points at the built Polygloss app", () => {
    expect(existsSync(appBin())).toBe(true);
  });
});
