import { afterAll, describe, expect, test } from "bun:test";
import { builtVersion, cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

describe("polygloss-cli", () => {
  test("polygloss-cli --version prints the built version", () => {
    const r = Bun.spawnSync([cliBin(), "--version"], { env: sandbox.env });
    expect(r.stderr.toString()).toBe("");
    expect(r.exitCode).toBe(0);
    expect(r.stdout.toString()).toBe(`polygloss ${builtVersion()}\n`);
  });
});
