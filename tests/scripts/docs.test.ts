// User docs stay true to the code (plan T5.5): the user guide's keymap and
// settings tables against the app's hidden `--dump-keymap --json` and
// `--dump-settings --json`, the agents doc against the MCP server's
// `tools/list` and resource templates and the CLI's JSON commands, and the
// commands README.md and CONTRIBUTING.md tell people to run.
import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { appBin, cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const read = (path: string) => readFileSync(join(repoRoot, path), "utf8");

// Building the app on a cold cache takes minutes.
const BUILD_TIMEOUT_MS = 30 * 60_000;

const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

function sandboxEnv(): Record<string, string> {
  const sandbox = makeSandbox();
  sandboxes.push(sandbox);
  return sandbox.env;
}

let appBuilt = false;

/**
 * Builds this checkout's `Polygloss` once (unless POLYGLOSS_SKIP_BUILD=1).
 *
 * `polygloss-cli` is built again right after, under the same lock: when
 * another worktree used the shared build dir since the preload's build,
 * scripts/cargo.sh first cleans this checkout's workspace members, which
 * removes `target/debug/polygloss-cli` that later suites run. One `-p` per
 * build, so the CLI never links the app's features.
 */
function buildApp(): void {
  if (appBuilt || process.env.POLYGLOSS_SKIP_BUILD === "1") return;
  const argv = [
    "scripts/cargo.sh",
    "with-lock",
    "/bin/sh",
    "-c",
    "scripts/cargo.sh build -p polygloss-app && scripts/cargo.sh build -p polygloss-cli",
  ];
  const r = Bun.spawnSync(argv, {
    cwd: repoRoot,
    env: process.env,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (r.exitCode !== 0)
    throw new Error(`\`${argv.join(" ")}\` failed with ${r.exitCode}`);
  appBuilt = true;
}

/** `Polygloss <flag> --json` in a sandbox, parsed. */
function appDump(flag: "--dump-keymap" | "--dump-settings"): any {
  buildApp();
  const r = Bun.spawnSync([appBin(), flag, "--json"], {
    env: sandboxEnv(),
    stdout: "pipe",
    stderr: "pipe",
  });
  if (r.exitCode !== 0)
    throw new Error(`Polygloss ${flag} --json: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString());
}

/** The section of `md` under the heading line `heading`, to the next heading of the same or a higher level. */
function section(md: string, heading: string): string {
  const lines = md.split("\n");
  const start = lines.indexOf(heading);
  if (start < 0) throw new Error(`no heading ${JSON.stringify(heading)}`);
  const level = heading.match(/^#+/)![0].length;
  const end = lines.findIndex(
    (line, i) =>
      i > start && /^#+ /.test(line) && line.match(/^#+/)![0].length <= level,
  );
  return lines.slice(start + 1, end < 0 ? undefined : end).join("\n");
}

/** The body rows of the first markdown table in `text`, as trimmed cells (`\|` unescaped). */
function tableRows(text: string): string[][] {
  const lines = text.split("\n");
  const start = lines.findIndex((l) => l.startsWith("|"));
  if (start < 0) throw new Error("no table");
  const rows: string[][] = [];
  for (const line of lines.slice(start + 2)) {
    if (!line.startsWith("|")) break;
    rows.push(
      line
        .slice(1, -1)
        .split(/(?<!\\)\|/)
        .map((cell) => cell.trim().replaceAll("\\|", "|")),
    );
  }
  return rows;
}

/** The code spans of a table cell, in order. */
function codeSpans(cell: string): string[] {
  return [...cell.matchAll(/`([^`]+)`/g)].map((m) => m[1]!);
}

/** Every leaf of a settings object as `[dotted.key, default]`. */
function settingsLeaves(value: any, prefix = ""): [string, unknown][] {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    return [[prefix, value]];
  return Object.entries(value).flatMap(([key, v]) =>
    settingsLeaves(v, prefix ? `${prefix}.${key}` : key),
  );
}

describe("docs/user-guide.md", () => {
  const guide = read("docs/user-guide.md");

  test(
    "user guide keymap table matches --dump-keymap",
    () => {
      const dump = appDump("--dump-keymap");
      const documented = tableRows(
        section(guide, "### Default key bindings"),
      ).map(([label, keys, title, action, context]) => ({
        label: codeSpans(label!).join(" "),
        keys: codeSpans(keys!)[0],
        title,
        action: codeSpans(action!)[0],
        context: context === "Anywhere" ? null : context,
      }));
      const expected = dump.bindings.map((b: any) => ({
        label: b.label,
        keys: b.keys,
        title: b.title,
        action: b.action,
        context: b.context,
      }));
      expect(documented).toEqual(expected);

      // Every action without a default key is listed too, so the guide
      // names every action `keymap.json` accepts.
      const bound = new Set(dump.bindings.map((b: any) => b.action));
      const unbound = tableRows(
        section(guide, "### Actions without a default key"),
      ).map(([title, action]) => ({ title, name: codeSpans(action!)[0] }));
      expect(unbound).toEqual(
        dump.actions
          .filter((a: any) => !bound.has(a.name))
          .map((a: any) => ({ title: a.title, name: a.name })),
      );
    },
    BUILD_TIMEOUT_MS,
  );

  test(
    "user guide lists every settings key",
    () => {
      const leaves = settingsLeaves(appDump("--dump-settings"));
      const rows = tableRows(section(guide, "### Settings keys"));
      const documented = new Map(
        rows.map(([key, dflt]) => [codeSpans(key!)[0]!, dflt!]),
      );
      expect(rows.length).toBe(documented.size); // no key twice
      expect([...documented.keys()].sort()).toEqual(
        leaves.map(([key]) => key).sort(),
      );
      for (const [key, value] of leaves) {
        const dflt = codeSpans(documented.get(key)!)[0];
        expect({
          key,
          value: dflt === undefined ? dflt : JSON.parse(dflt),
        }).toEqual({
          key,
          value,
        });
      }
    },
    BUILD_TIMEOUT_MS,
  );

  test("links in the new docs resolve inside the repo", () => {
    for (const doc of [
      "docs/user-guide.md",
      "docs/agents.md",
      "CONTRIBUTING.md",
    ]) {
      const text = read(doc);
      for (const [, target] of text.matchAll(/\]\(([^)#]+)(#[^)]*)?\)/g)) {
        if (/^[a-z]+:/.test(target!)) continue; // external
        const path = join(repoRoot, dirname(doc), target!);
        expect({ doc, target, exists: existsSync(path) }).toEqual({
          doc,
          target,
          exists: true,
        });
      }
    }
  });
});

describe("docs/agents.md", () => {
  const agents = read("docs/agents.md");

  test("agents doc names every MCP tool from listTools", async () => {
    const { client, close } = await connectMcp({ env: sandboxEnv() });
    try {
      const { tools } = await client.listTools();
      const documented = tableRows(section(agents, "### Tools")).map(
        ([name]) => codeSpans(name!)[0],
      );
      expect(documented.sort()).toEqual(tools.map((t) => t.name).sort());

      const { resourceTemplates } = await client.listResourceTemplates();
      const uris = tableRows(section(agents, "### Resources")).map(
        ([uri]) => codeSpans(uri!)[0],
      );
      expect(uris.sort()).toEqual(
        resourceTemplates.map((t) => t.uriTemplate).sort(),
      );
    } finally {
      await close();
    }
  });

  test("agents doc maps every JSON CLI command to its MCP tool", () => {
    const r = Bun.spawnSync([cliBin(), "--help"], {
      env: sandboxEnv(),
      stdout: "pipe",
    });
    expect(r.exitCode).toBe(0);
    // `  reviews      JSON CLI: list reviews (MCP `list_reviews`)`
    const fromHelp = [
      ...r.stdout
        .toString()
        .matchAll(/^ {2}([a-z-]+) +JSON CLI: .*\(MCP `([a-z_]+)`\)$/gm),
    ].map(([, command, tool]) => ({ command, tool }));
    expect(fromHelp.length).toBeGreaterThan(10);
    const documented = tableRows(section(agents, "## JSON CLI")).map(
      ([command, tool]) => ({
        command: codeSpans(command!)[0]!.split(" ")[1],
        tool: codeSpans(tool!)[0],
      }),
    );
    for (const pair of fromHelp) expect(documented).toContainEqual(pair);
  });
});

describe("README.md and CONTRIBUTING.md", () => {
  const scripts = Object.keys(
    (JSON.parse(read("package.json")) as { scripts: Record<string, string> })
      .scripts,
  );

  /** `bun run <script>` and `scripts/<file>` mentioned in `doc`. */
  function commands(doc: string): { runs: string[]; files: string[] } {
    const text = read(doc);
    return {
      runs: [...text.matchAll(/\bbun run ([a-z0-9:-]+)/g)].map((m) => m[1]!),
      files: [
        ...text.matchAll(/(?<![\w./-])(scripts\/[a-z0-9./-]+[a-z0-9])/g),
      ].map((m) => m[1]!),
    };
  }

  test("readme commands exist in package json", () => {
    const { runs, files } = commands("README.md");
    expect(runs.length).toBeGreaterThan(0);
    for (const run of runs) expect(scripts).toContain(run);
    for (const file of files)
      expect({ file, exists: existsSync(join(repoRoot, file)) }).toEqual({
        file,
        exists: true,
      });
  });

  test("contributing commands exist in package json and scripts/", () => {
    const { runs, files } = commands("CONTRIBUTING.md");
    expect(runs).toEqual(
      expect.arrayContaining(["format", "lint", "test:unit", "test:e2e"]),
    );
    for (const run of runs) expect(scripts).toContain(run);
    for (const file of files)
      expect({ file, exists: existsSync(join(repoRoot, file)) }).toEqual({
        file,
        exists: true,
      });
  });
});
