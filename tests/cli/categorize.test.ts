// `polygloss debug categorize [--repo <path>] [--json] <path>…` (T6.9,
// design §11.15): explains each path's file category from the sandbox's
// settings.json, and with --repo from that repo's `linguist-generated`.
import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

/** A fresh sandbox whose settings.json is `settings`. */
function world(settings: string): {
  env: Record<string, string>;
  home: string;
} {
  const sandbox = makeSandbox();
  sandboxes.push(sandbox);
  const config = join(sandbox.configDir, "polygloss");
  mkdirSync(config, { recursive: true });
  writeFileSync(join(config, "settings.json"), settings);
  return { env: sandbox.env, home: sandbox.home };
}

function categorize(
  env: Record<string, string>,
  args: string[],
): { json: any; stderr: string; exitCode: number } {
  const r = Bun.spawnSync(
    [cliBin(), "debug", "categorize", "--json", ...args],
    { env },
  );
  const stdout = r.stdout.toString();
  return {
    json: stdout.trim() === "" ? null : JSON.parse(stdout),
    stderr: r.stderr.toString(),
    exitCode: r.exitCode,
  };
}

describe("polygloss debug categorize", () => {
  test("debug categorize explains built-in, extra, custom, attribute and rescued verdicts", () => {
    const { env, home } = world(
      JSON.stringify({
        categories: {
          tests: { patterns: ["qa/", "!tests/fixtures/"] },
          custom: [
            {
              id: "tokens",
              name: "Design tokens",
              patterns: ["*.tokens.json"],
            },
          ],
        },
      }),
    );
    // A repo whose HEAD marks gen.txt generated.
    const repo = join(home, "repo");
    mkdirSync(repo);
    const git = (args: string[]) => {
      const r = Bun.spawnSync(["git", "-C", repo, ...args], { env });
      if (r.exitCode !== 0) throw new Error(r.stderr.toString());
    };
    git(["init", "-q", "-b", "main"]);
    writeFileSync(join(repo, ".gitattributes"), "gen.txt linguist-generated\n");
    git(["add", "."]);
    git(["commit", "-q", "-m", "attrs"]);

    const paths = [
      "src/a.test.ts",
      "qa/smoke.sh",
      "ui/colors.tokens.json",
      "tests/fixtures/x.json",
      "gen.txt",
      "src/main.rs",
    ];
    const withRepo = categorize(env, ["--repo", repo, ...paths]);
    expect(withRepo.stderr).toBe("");
    expect(withRepo.exitCode).toBe(0);
    expect(withRepo.json).toEqual([
      {
        path: "src/a.test.ts",
        category: "tests",
        title: "Tests",
        source: "built-in",
        group: "unit",
        pattern: "*.test.*",
      },
      {
        path: "qa/smoke.sh",
        category: "tests",
        title: "Tests",
        source: "extra",
        pattern: "qa/",
      },
      {
        path: "ui/colors.tokens.json",
        category: "custom:tokens",
        title: "Design tokens",
        source: "custom",
        pattern: "*.tokens.json",
      },
      {
        path: "tests/fixtures/x.json",
        category: null,
        rescued_by: [{ category: "tests", pattern: "!tests/fixtures/" }],
      },
      {
        path: "gen.txt",
        category: "generated",
        title: "Generated",
        source: "attribute",
        pattern: "linguist-generated",
      },
      { path: "src/main.rs", category: null },
    ]);

    // Without --repo the attribute is unspecified: gen.txt is uncategorized.
    const bare = categorize(env, ["gen.txt", "src/a.test.ts"]);
    expect(bare.exitCode).toBe(0);
    expect(bare.json[0]).toEqual({ path: "gen.txt", category: null });
    expect(bare.json[1].category).toBe("tests");
  });

  test("debug categorize reports an invalid categories section", () => {
    const { env } = world(
      JSON.stringify({ categories: { tests: { patterns: ["[abc"] } } }),
    );
    const r = categorize(env, ["src/a.test.ts"]);
    expect(r.exitCode).toBe(1);
    expect(r.json.error.code).toBe("conflict");
    expect(r.json.error.message).toStartWith("settings.json: ");
    expect(r.json.error.message).toContain("[abc");
  });
});
