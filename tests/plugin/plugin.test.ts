// The Claude Code plugin and its marketplace (T4.10, design §16.1 and §16.3):
// the manifests are validated statically, the shim's resolution order is
// driven with fake CLIs in a sandbox HOME/PATH, and the `.mcp.json` server and
// the Stop hook command are run the way Claude Code runs them (through the
// shim, against this checkout's `polygloss-cli`). Nothing here installs the
// plugin or touches the real Claude Code config: `claude plugin validate` only
// runs with a sandboxed HOME and CLAUDE_CONFIG_DIR, and is skipped when the
// `claude` CLI is absent.
import { afterAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { cliBin } from "../support/bins";
import { expectedTools } from "../support/mcp";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const pluginDir = join(repoRoot, "plugins/polygloss");
const shimPath = join(pluginDir, "bin/polygloss-shim");
const appCli = "/Applications/Polygloss.app/Contents/MacOS/polygloss-cli";

const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

function readJson(path: string): Record<string, unknown> {
  return JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
}

/** The first ```json block after `heading` in docs/design.md. */
function designJsonBlock(heading: string): unknown {
  const design = readFileSync(join(repoRoot, "docs/design.md"), "utf8");
  const start = design.indexOf(heading);
  expect(start).toBeGreaterThanOrEqual(0);
  const block = /```json\n([\s\S]*?)\n```/.exec(design.slice(start));
  if (!block?.[1]) throw new Error(`no json block after ${heading}`);
  return JSON.parse(block[1]);
}

/** Claude Code substitutes `${CLAUDE_PLUGIN_ROOT}` before spawning. */
function substituteRoot(value: string, root: string): string {
  return value.replaceAll("${CLAUDE_PLUGIN_ROOT}", root);
}

/**
 * A sandbox with a copy of the plugin whose shim looks for the app bundle
 * under the sandbox instead of the real /Applications, so no test depends on
 * (or could run) an installed Polygloss.
 */
function pluginSandbox(): {
  home: string;
  dataDir: string;
  root: string;
  pluginRoot: string;
  shim: string;
  appCli: string;
  pathDir: string;
  env: Record<string, string>;
} {
  const sandbox = makeSandbox();
  sandboxes.push(sandbox);
  const root = resolve(sandbox.home, "..");
  const pluginRoot = join(root, "plugin");
  cpSync(pluginDir, pluginRoot, { recursive: true });
  const shim = join(pluginRoot, "bin/polygloss-shim");
  const source = readFileSync(shim, "utf8");
  const fakeApps = join(root, "Applications");
  writeFileSync(
    shim,
    source.replaceAll(
      "/Applications/Polygloss.app",
      `${fakeApps}/Polygloss.app`,
    ),
  );
  chmodSync(shim, 0o755);
  const pathDir = join(root, "path-bin");
  mkdirSync(pathDir, { recursive: true });
  return {
    home: sandbox.home,
    dataDir: sandbox.dataDir,
    root,
    pluginRoot,
    shim,
    appCli: `${fakeApps}/Polygloss.app/Contents/MacOS/polygloss-cli`,
    pathDir,
    // The fake PATH dir first, then only system dirs: never a real polygloss.
    env: { ...sandbox.env, PATH: `${pathDir}:/usr/bin:/bin` },
  };
}

/** Writes an executable that prints `identity`, its pid, its argv and stdin. */
function fakeCli(path: string, identity: string): void {
  mkdirSync(resolve(path, ".."), { recursive: true });
  writeFileSync(
    path,
    [
      "#!/bin/bash",
      `echo "${identity}"`,
      'echo "pid=$$"',
      'for a in "$@"; do echo "arg=$a"; done',
      'if read -r line; then echo "stdin=$line"; fi',
      "",
    ].join("\n"),
  );
  chmodSync(path, 0o755);
}

function runShim(opts: {
  shim: string;
  env: Record<string, string>;
  args?: string[];
  stdin?: string;
}): { exitCode: number; stdout: string; stderr: string; pid: number } {
  const r = Bun.spawnSync([opts.shim, ...(opts.args ?? ["mcp"])], {
    env: opts.env,
    stdin: new Blob([opts.stdin ?? ""]),
  });
  return {
    exitCode: r.exitCode,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
    pid: r.pid,
  };
}

describe("marketplace and manifests", () => {
  test("marketplace lists the polygloss plugin with a relative source", () => {
    const marketplace = readJson(
      join(repoRoot, ".claude-plugin/marketplace.json"),
    );
    expect(marketplace.name).toBe("polygloss");
    expect((marketplace.owner as { name?: unknown }).name).toBeString();
    expect(marketplace.description).toBeString();
    const plugins = marketplace.plugins as { name: string; source: unknown }[];
    expect(plugins.map((p) => p.name)).toEqual(["polygloss"]);
    const source = plugins[0]?.source;
    // A path from the marketplace root: `./`, no `..`, forward slashes.
    expect(source).toBe("./plugins/polygloss");
    expect(
      existsSync(
        join(repoRoot, source as string, ".claude-plugin/plugin.json"),
      ),
    ).toBe(true);
  });

  test("plugin json is valid", () => {
    const manifest = readJson(join(pluginDir, ".claude-plugin/plugin.json"));
    expect(manifest.name).toBe("polygloss");
    expect(manifest.description).toBeString();
    expect((manifest.author as { name?: unknown }).name).toBeString();
    expect(manifest.license).toBe("MIT OR Apache-2.0");
    // The plugin version follows the workspace's (Cargo.toml).
    const cargo = readFileSync(join(repoRoot, "Cargo.toml"), "utf8");
    const version = /\[workspace\.package\][^[]*?\nversion = "([^"]+)"/.exec(
      cargo,
    )?.[1];
    expect(manifest.version).toBe(version);
    // Components live in their default locations; the manifest only describes.
    for (const key of ["mcpServers", "hooks", "skills", "commands", "agents"]) {
      expect({ key, present: key in manifest }).toEqual({
        key,
        present: false,
      });
    }
    // Only documented top-level keys (Claude Code strips unknown ones).
    const known = new Set([
      "$schema",
      "name",
      "displayName",
      "version",
      "description",
      "author",
      "homepage",
      "repository",
      "license",
      "keywords",
    ]);
    expect(Object.keys(manifest).filter((k) => !known.has(k))).toEqual([]);
  });

  test("mcp json runs the shim with mcp", () => {
    // Plugin `.mcp.json` wraps the servers in `mcpServers` (Claude Code docs,
    // "Plugin-provided MCP servers"). No `--channel`: that stays opt-in (§16.4).
    expect(readJson(join(pluginDir, ".mcp.json"))).toEqual({
      mcpServers: {
        polygloss: {
          command: "${CLAUDE_PLUGIN_ROOT}/bin/polygloss-shim",
          args: ["mcp"],
        },
      },
    });
  });

  test("hooks json equals the design block", () => {
    const hooks = readJson(join(pluginDir, "hooks/hooks.json"));
    expect(hooks).toEqual(
      designJsonBlock("### 16.3 Hook and waiter") as Record<string, unknown>,
    );
    const hook = (
      hooks.hooks as { Stop: { hooks: Record<string, unknown>[] }[] }
    ).Stop[0]?.hooks[0];
    expect(hook).toEqual({
      type: "command",
      command:
        '"${CLAUDE_PLUGIN_ROOT}/bin/polygloss-shim" wait --session "$CLAUDE_CODE_SESSION_ID"',
      asyncRewake: true,
      timeout: 3600,
    });
  });

  test("skill has name and description frontmatter", () => {
    const skill = readFileSync(
      join(pluginDir, "skills/review-loop/SKILL.md"),
      "utf8",
    );
    // Claude Code reads frontmatter only when `---` is the first line.
    const match = /^---\n([\s\S]*?)\n---\n([\s\S]*)$/.exec(skill);
    if (!match) throw new Error("SKILL.md has no frontmatter");
    const frontmatter = Bun.YAML.parse(match[1] ?? "") as Record<
      string,
      unknown
    >;
    expect(frontmatter.name).toBe("review-loop");
    const description = String(frontmatter.description ?? "");
    expect(description.length).toBeGreaterThan(40);
    // The skill listing truncates description (+ when_to_use) at 1,536 chars.
    expect(description.length).toBeLessThan(1536);
    expect(description).toContain("Polygloss");
    expect(description).toContain(
      "Polygloss: the human submitted their review",
    );
  });

  test("every SKILL.md frontmatter is strict YAML", () => {
    // A plain scalar with ": " in it is invalid YAML that lenient parsers
    // accept: quote such descriptions (T5.9).
    const skillsDir = join(pluginDir, "skills");
    const skills = readdirSync(skillsDir).map((d) =>
      join(skillsDir, d, "SKILL.md"),
    );
    expect(skills.length).toBeGreaterThan(0);
    for (const path of skills) {
      const text = readFileSync(path, "utf8");
      const match = /^---\n([\s\S]*?)\n---\n/.exec(text);
      if (!match) throw new Error(`${path} has no frontmatter`);
      const parsed = Bun.YAML.parse(match[1] ?? "") as Record<string, unknown>;
      expect(typeof parsed.name).toBe("string");
      expect(typeof parsed.description).toBe("string");
    }
  });

  test("skill names every position state", () => {
    const body = readFileSync(
      join(pluginDir, "skills/review-loop/SKILL.md"),
      "utf8",
    );
    for (const state of ["exact", "moved", "outdated", "absent"]) {
      expect({ state, found: body.includes(`\`${state}\``) }).toEqual({
        state,
        found: true,
      });
    }
  });

  test("skill teaches the design 15.4 loop", () => {
    const body = readFileSync(
      join(pluginDir, "skills/review-loop/SKILL.md"),
      "utf8",
    );
    // Every step of the server instructions' loop, in order.
    const steps = [
      "open_diff",
      "create_comment",
      "wait_for_review",
      "list_threads",
      "get_thread",
      "reply",
      "resolve",
      "request_rereview",
    ];
    let at = 0;
    for (const step of steps) {
      const found = body.indexOf(step, at);
      expect({ step, found: found >= 0 }).toEqual({ step, found: true });
      at = found;
    }
    for (const topic of [
      "```suggestion",
      "next_cursor",
      "focus",
      "approve",
      "request_changes",
    ]) {
      expect({ topic, found: body.includes(topic) }).toEqual({
        topic,
        found: true,
      });
    }
  });

  test("the shim is an executable bash script with the design paths", () => {
    expect(statSync(shimPath).mode & 0o111).toBe(0o111);
    const source = readFileSync(shimPath, "utf8");
    expect(source.startsWith("#!/bin/bash\n")).toBe(true);
    expect(source).toContain("Library/Application Support/polygloss");
    expect(source).toContain(appCli);
    // git records it as executable too, so a marketplace clone can run it.
    const r = Bun.spawnSync(
      ["git", "ls-files", "-s", "plugins/polygloss/bin/polygloss-shim"],
      {
        cwd: repoRoot,
        env: {
          PATH: process.env.PATH ?? "/usr/bin:/bin",
          GIT_CONFIG_NOSYSTEM: "1",
        },
      },
    );
    const staged = r.stdout.toString();
    if (staged !== "") expect(staged.startsWith("100755 ")).toBe(true);
  });
});

describe("polygloss-shim", () => {
  test("shim resolution order", () => {
    const s = pluginSandbox();
    const stable = join(s.dataDir, "bin/polygloss");
    fakeCli(stable, "stable");
    fakeCli(join(s.pathDir, "polygloss"), "path");
    fakeCli(s.appCli, "bundle");

    // 1. The stable symlink path in the data dir.
    expect(runShim({ shim: s.shim, env: s.env }).stdout.split("\n")[0]).toBe(
      "stable",
    );
    // 2. Then `polygloss` on PATH.
    Bun.spawnSync(["rm", stable]);
    expect(runShim({ shim: s.shim, env: s.env }).stdout.split("\n")[0]).toBe(
      "path",
    );
    // 3. Then the app bundle's CLI.
    Bun.spawnSync(["rm", join(s.pathDir, "polygloss")]);
    expect(runShim({ shim: s.shim, env: s.env }).stdout.split("\n")[0]).toBe(
      "bundle",
    );
  });

  test("without POLYGLOSS_DATA_DIR the stable path is under HOME Library", () => {
    const s = pluginSandbox();
    const env = { ...s.env };
    delete env.POLYGLOSS_DATA_DIR;
    fakeCli(
      join(s.home, "Library/Application Support/polygloss/bin/polygloss"),
      "home-stable",
    );
    fakeCli(join(s.pathDir, "polygloss"), "path");
    expect(runShim({ shim: s.shim, env }).stdout.split("\n")[0]).toBe(
      "home-stable",
    );
    // POLYGLOSS_DATA_DIR moves it, like every other data-dir path.
    fakeCli(join(s.dataDir, "bin/polygloss"), "data-dir-stable");
    expect(runShim({ shim: s.shim, env: s.env }).stdout.split("\n")[0]).toBe(
      "data-dir-stable",
    );
  });

  test("a dangling stable symlink falls through to PATH", () => {
    // The app refreshes the link at launch; a deleted app leaves it dangling.
    const s = pluginSandbox();
    mkdirSync(join(s.dataDir, "bin"), { recursive: true });
    symlinkSync(
      join(s.root, "gone/polygloss-cli"),
      join(s.dataDir, "bin/polygloss"),
    );
    fakeCli(join(s.pathDir, "polygloss"), "path");
    const r = runShim({ shim: s.shim, env: s.env });
    expect(r.exitCode).toBe(0);
    expect(r.stdout.split("\n")[0]).toBe("path");
  });

  test("shim execs the CLI in place with argv and stdin untouched", () => {
    const s = pluginSandbox();
    fakeCli(join(s.dataDir, "bin/polygloss"), "stable");
    const r = runShim({
      shim: s.shim,
      env: s.env,
      args: ["wait", "--session", "", "two words", "$HOME"],
      stdin: '{"session_id":"abc"}\n',
    });
    expect(r.exitCode).toBe(0);
    expect(r.stderr).toBe("");
    expect(r.stdout).toBe(
      [
        "stable",
        `pid=${r.pid}`,
        "arg=wait",
        "arg=--session",
        "arg=",
        "arg=two words",
        "arg=$HOME",
        'stdin={"session_id":"abc"}',
        "",
      ].join("\n"),
    );
  });

  test("shim exits 0 with a one-line hint when nothing is found", () => {
    // Not 127: the Stop hook would report an error on every turn of every
    // session with the plugin enabled but Polygloss not installed (T5.9).
    const s = pluginSandbox();
    for (const args of [["mcp"], ["wait", "--session", "s"]]) {
      const r = runShim({ shim: s.shim, env: s.env, args });
      expect(r.exitCode).toBe(0);
      // stdout is the MCP JSON-RPC channel: the shim never writes to it.
      expect(r.stdout).toBe("");
      const lines = r.stderr.trimEnd().split("\n");
      expect(lines).toHaveLength(1);
      expect(lines[0]).toContain("polygloss-shim: Polygloss is not installed");
      expect(lines[0]).toContain(join(s.dataDir, "bin/polygloss"));
      expect(lines[0]).toContain("polygloss on PATH");
      expect(lines[0]).toContain(s.appCli);
    }
  });

  test("a relative POLYGLOSS_DATA_DIR is an error, as it is for the CLI", () => {
    const s = pluginSandbox();
    fakeCli(join(s.pathDir, "polygloss"), "path");
    const r = runShim({
      shim: s.shim,
      env: { ...s.env, POLYGLOSS_DATA_DIR: "relative/data" },
    });
    expect(r.exitCode).toBe(1);
    expect(r.stdout).toBe("");
    expect(r.stderr).toContain("POLYGLOSS_DATA_DIR must be an absolute path");
    // Empty counts as unset (like core): falls through to the HOME path, then PATH.
    const empty = runShim({
      shim: s.shim,
      env: { ...s.env, POLYGLOSS_DATA_DIR: "" },
    });
    expect(empty.exitCode).toBe(0);
    expect(empty.stdout.split("\n")[0]).toBe("path");
  });
});

describe("the plugin as Claude Code runs it", () => {
  test("mcp json starts polygloss mcp through the shim", async () => {
    const s = pluginSandbox();
    mkdirSync(join(s.dataDir, "bin"), { recursive: true });
    symlinkSync(cliBin(), join(s.dataDir, "bin/polygloss"));
    const server = (
      readJson(join(s.pluginRoot, ".mcp.json")).mcpServers as Record<
        string,
        { command: string; args: string[] }
      >
    ).polygloss;
    if (!server) throw new Error("no polygloss server in .mcp.json");
    const transport = new StdioClientTransport({
      command: substituteRoot(server.command, s.pluginRoot),
      args: server.args.map((a) => substituteRoot(a, s.pluginRoot)),
      env: { ...s.env, CLAUDE_PLUGIN_ROOT: s.pluginRoot },
      stderr: "pipe",
    });
    const client = new Client({ name: "claude-code", version: "0.0.0" });
    await client.connect(transport);
    try {
      expect(client.getServerVersion()?.name).toBe("polygloss");
      const { tools } = await client.listTools();
      expect(tools.map((t) => t.name).sort()).toEqual(
        [...expectedTools].sort(),
      );
    } finally {
      await client.close();
    }
  });

  test("hook command passes wait and the session id to the CLI", () => {
    const s = pluginSandbox();
    fakeCli(join(s.dataDir, "bin/polygloss"), "stable");
    const hooks = readJson(join(s.pluginRoot, "hooks/hooks.json"));
    const hook = (hooks.hooks as { Stop: { hooks: { command: string }[] }[] })
      .Stop[0]?.hooks[0];
    if (!hook) throw new Error("no Stop hook");
    const command = substituteRoot(hook.command, s.pluginRoot);
    // Claude Code runs shell-form hooks with `sh -c` on macOS.
    const run = (env: Record<string, string>) =>
      Bun.spawnSync(["/bin/sh", "-c", command], {
        env: { ...env, CLAUDE_PLUGIN_ROOT: s.pluginRoot },
        stdin: new Blob(['{"session_id":"from-stdin"}\n']),
      });
    const withId = run({ ...s.env, CLAUDE_CODE_SESSION_ID: "session-123" });
    expect(withId.exitCode).toBe(0);
    expect(
      withId.stdout
        .toString()
        .split("\n")
        .filter((l) => l.startsWith("arg=")),
    ).toEqual(["arg=wait", "arg=--session", "arg=session-123"]);
    // Unset: an empty --session, which `polygloss wait` treats as unset and
    // falls back to the stdin session_id (T4.8).
    const withoutId = run(s.env);
    const lines = withoutId.stdout.toString().split("\n");
    expect(lines.filter((l) => l.startsWith("arg="))).toEqual([
      "arg=wait",
      "arg=--session",
      "arg=",
    ]);
    expect(lines).toContain('stdin={"session_id":"from-stdin"}');
  });

  test("hook command runs polygloss wait and exits 0 with nothing assigned", () => {
    const s = pluginSandbox();
    mkdirSync(join(s.dataDir, "bin"), { recursive: true });
    symlinkSync(cliBin(), join(s.dataDir, "bin/polygloss"));
    const hooks = readJson(join(s.pluginRoot, "hooks/hooks.json"));
    const hook = (hooks.hooks as { Stop: { hooks: { command: string }[] }[] })
      .Stop[0]?.hooks[0];
    if (!hook) throw new Error("no Stop hook");
    const r = Bun.spawnSync(
      ["/bin/sh", "-c", substituteRoot(hook.command, s.pluginRoot)],
      {
        env: {
          ...s.env,
          CLAUDE_PLUGIN_ROOT: s.pluginRoot,
          CLAUDE_CODE_SESSION_ID: "plugin-test-session",
        },
        stdin: new Blob([
          '{"session_id":"plugin-test-session","hook_event_name":"Stop"}\n',
        ]),
      },
    );
    // No open review is assigned to the session: exit 0 at once (§16.3 step 2).
    expect({ exitCode: r.exitCode, stderr: r.stderr.toString() }).toEqual({
      exitCode: 0,
      stderr: "",
    });
  });
});

// `claude plugin validate` is the authoritative check (Claude Code docs). It
// runs only with a sandboxed HOME and CLAUDE_CONFIG_DIR, never the real config.
const claudeCli = Bun.which("claude");
describe.skipIf(claudeCli === null)("claude plugin validate", () => {
  function validate(target: string): { exitCode: number; output: string } {
    const sandbox = makeSandbox();
    sandboxes.push(sandbox);
    const r = Bun.spawnSync(
      [claudeCli as string, "plugin", "validate", target, "--strict"],
      {
        cwd: sandbox.home,
        env: {
          ...sandbox.env,
          CLAUDE_CONFIG_DIR: join(sandbox.home, ".claude"),
          DISABLE_AUTOUPDATER: "1",
          DISABLE_TELEMETRY: "1",
          DISABLE_ERROR_REPORTING: "1",
          CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
        },
        timeout: 60_000,
      },
    );
    return {
      exitCode: r.exitCode,
      output: `${r.stdout.toString()}${r.stderr.toString()}`,
    };
  }

  test("marketplace passes strict validation", () => {
    const r = validate(repoRoot);
    expect({ exitCode: r.exitCode, output: r.output }).toMatchObject({
      exitCode: 0,
    });
    expect(r.output).toContain("Validation passed");
  }, 90_000);

  test("plugin passes strict validation", () => {
    const r = validate(pluginDir);
    expect({ exitCode: r.exitCode, output: r.output }).toMatchObject({
      exitCode: 0,
    });
    expect(r.output).toContain("Validation passed");
  }, 90_000);
});
