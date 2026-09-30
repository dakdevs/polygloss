// The manual wake-up gate kit (T4.12, plan "Manual gate"): the human runs
// scripts/wake-gate/prepare.sh, then follows docs/testing/agent-wake-gate.md
// in real Claude Code. These tests pin the script against a fake rustup cargo
// (nothing is built), a sandboxed gate root and HOME, and the real CLI for a
// W1 rehearsal without Claude. `claude` itself only ever runs with a sandbox
// HOME and CLAUDE_CONFIG_DIR, never against the real Claude Code config.
import { Database } from "bun:sqlite";
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const prepare = join(repoRoot, "scripts", "wake-gate", "prepare.sh");
const gateDoc = join(repoRoot, "docs", "testing", "agent-wake-gate.md");
const pluginDir = join(repoRoot, "plugins", "polygloss");
const sandbox = makeSandbox();
let cargoHome = "";
let rootCount = 0;

// A fake rustup cargo: logs argv and "builds" the two binaries. The CLI is a
// symlink to the real polygloss-cli when FAKE_REAL_CLI is set (in a target dir
// of its own), else a script that prints its identity.
const fakeCargo = `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_CARGO_LOG"
out="$CARGO_TARGET_DIR/debug"
case "$*" in
  "build -p polygloss-app")
    mkdir -p "$out" && printf '#!/bin/sh\\necho fake-polygloss-app\\n' >"$out/Polygloss" && chmod +x "$out/Polygloss" ;;
  "build -p polygloss-cli")
    # Never write through a previous run's link to the real CLI.
    mkdir -p "$out" && rm -f "$out/polygloss-cli"
    if [ -n "\${FAKE_REAL_CLI-}" ]; then ln -sfn "$FAKE_REAL_CLI" "$out/polygloss-cli"
    else printf '#!/bin/sh\\necho "fake-polygloss-cli $*"\\n' >"$out/polygloss-cli" && chmod +x "$out/polygloss-cli"; fi ;;
esac
`;

beforeAll(() => {
  cargoHome = join(sandbox.home, "cargo-home");
  mkdirSync(join(cargoHome, "bin"), { recursive: true });
  writeFileSync(join(cargoHome, "bin", "cargo"), fakeCargo);
  chmodSync(join(cargoHome, "bin", "cargo"), 0o755);
});

afterAll(() => sandbox.cleanup());

type Run = {
  exitCode: number;
  stdout: string;
  stderr: string;
  output: string;
  cargoLog: string[];
  root: string;
  targetDir: string;
};

/** A fresh (not yet existing) gate root inside the sandbox. */
function newRoot(): string {
  rootCount += 1;
  return join(sandbox.home, `gate-${rootCount}`);
}

function runPrepare(
  args: string[],
  opts: { root?: string; env?: Record<string, string> } = {},
): Run {
  const root = opts.root ?? newRoot();
  const targetDir = join(sandbox.home, "target");
  const cargoLog = join(sandbox.home, `cargo-${rootCount}.log`);
  rmSync(cargoLog, { force: true });
  const r = Bun.spawnSync([prepare, "--root", root, ...args], {
    cwd: sandbox.home,
    env: {
      ...sandbox.env,
      CARGO_HOME: cargoHome,
      CARGO_TARGET_DIR: targetDir,
      CARGO_BUILD_BUILD_DIR: join(sandbox.home, "target-shared"),
      FAKE_CARGO_LOG: cargoLog,
      ...opts.env,
    },
    timeout: 60_000,
  });
  const stdout = r.stdout.toString();
  const stderr = r.stderr.toString();
  return {
    exitCode: r.exitCode ?? -1,
    stdout,
    stderr,
    output: stdout + stderr,
    cargoLog: existsSync(cargoLog)
      ? readFileSync(cargoLog, "utf8").split("\n").filter(Boolean)
      : [],
    root,
    targetDir,
  };
}

function git(repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env: sandbox.env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
  return r.stdout.toString();
}

describe("scripts/wake-gate/prepare.sh", () => {
  test("prepare script dry run lists every step", () => {
    const r = runPrepare(["--dry-run"]);
    expect(r.stderr).toBe("");
    expect(r.exitCode).toBe(0);
    const data = join(r.root, "data");
    const cli = join(r.targetDir, "debug", "polygloss-cli");
    const app = join(r.targetDir, "debug", "Polygloss");
    for (const step of [
      "dry run: nothing is built or written",
      "step 1/6: build the app: scripts/cargo.sh build -p polygloss-app",
      "step 2/6: build the CLI: scripts/cargo.sh build -p polygloss-cli",
      `step 3/6: create the gate root ${r.root} with the sandbox data dir ${data} (0700)`,
      `step 4/6: link ${data}/bin/polygloss -> ${cli}`,
      `step 5/6: create scratch repo A ${r.root}/repo-a (one commit, one uncommitted change to src/scale.js)`,
      `step 6/6: create scratch repo B ${r.root}/repo-b (the same, for W7)`,
      // The human's commands.
      "claude --version",
      "sw_vers -productVersion",
      `export POLYGLOSS_DATA_DIR=${data}`,
      "export POLYGLOSS_TEST=1",
      `export POLYGLOSS_APP_BIN=${app}`,
      'export PATH="$POLYGLOSS_DATA_DIR/bin:$PATH"',
      `cd ${r.root}/repo-a`,
      `claude plugin marketplace add ${repoRoot} --scope local`,
      "claude plugin install polygloss@polygloss --scope local",
      "export POLYGLOSS_WAIT_TIMEOUT_S=120",
      `cd ${r.root}/repo-b`,
      "polygloss reviews --json",
      "pgrep -fl 'polygloss(-cli)? wait'",
      `sqlite3 "$POLYGLOSS_DATA_DIR/polygloss.db" 'select session_id, pid from waiters'`,
      "claude plugin uninstall polygloss@polygloss --scope local",
      "claude plugin marketplace remove polygloss",
      "docs/testing/agent-wake-gate.md",
    ])
      expect(r.stdout).toContain(step);
    // A dry run builds nothing and writes nothing.
    expect(r.cargoLog).toEqual([]);
    expect(existsSync(r.root)).toBe(false);
  });

  test("--help names the default gate root and every option", () => {
    const r = Bun.spawnSync([prepare, "--help"], { env: sandbox.env });
    const out = r.stdout.toString();
    expect(r.exitCode).toBe(0);
    for (const s of [
      "/tmp/polygloss-wake-gate",
      "--dry-run",
      "--root",
      "--no-build",
    ])
      expect(out).toContain(s);
  });

  test("an unknown option is a usage error", () => {
    const r = runPrepare(["--bogus"]);
    expect(r.exitCode).toBe(2);
    expect(r.stderr).toContain("--bogus");
    expect(existsSync(r.root)).toBe(false);
  });

  test("builds both binaries, links the stable CLI path and creates the scratch repos", () => {
    const r = runPrepare([]);
    expect({ exitCode: r.exitCode, output: r.output }).toMatchObject({
      exitCode: 0,
    });
    // One -p per build (a joint build would unify features into the CLI).
    expect(r.cargoLog).toEqual([
      "build -p polygloss-app",
      "build -p polygloss-cli",
    ]);
    const data = join(r.root, "data");
    expect(statSync(data).mode & 0o777).toBe(0o700);
    const link = join(data, "bin", "polygloss");
    expect(lstatSync(link).isSymbolicLink()).toBe(true);
    expect(readlinkSync(link)).toBe(
      join(r.targetDir, "debug", "polygloss-cli"),
    );
    // The plugin's shim finds the CLI through the stable link.
    const shim = Bun.spawnSync(
      [join(pluginDir, "bin", "polygloss-shim"), "--version"],
      { env: { ...sandbox.env, POLYGLOSS_DATA_DIR: data } },
    );
    expect(shim.stdout.toString().trim()).toBe("fake-polygloss-cli --version");

    for (const name of ["repo-a", "repo-b"]) {
      const repo = join(r.root, name);
      expect(git(repo, ["rev-list", "--count", "HEAD"]).trim()).toBe("1");
      expect(git(repo, ["status", "--porcelain"])).toBe(" M src/scale.js\n");
      // Plugin settings written by `--scope local` never enter the review.
      mkdirSync(join(repo, ".claude"), { recursive: true });
      writeFileSync(join(repo, ".claude", "settings.local.json"), "{}\n");
      expect(git(repo, ["status", "--porcelain"])).toBe(" M src/scale.js\n");
      // W1's prompt renames `x`.
      expect(readFileSync(join(repo, "src", "scale.js"), "utf8")).toMatch(
        /\bx\b/,
      );
    }
    expect(r.stdout).toContain("ready:");
  });

  test("a rerun replaces the previous gate root", () => {
    const first = runPrepare([]);
    expect(first.exitCode).toBe(0);
    const leftover = join(first.root, "data", "polygloss.db");
    writeFileSync(leftover, "old state");
    writeFileSync(join(first.root, "repo-a", "src", "scale.js"), "edited\n");
    const again = runPrepare([], { root: first.root });
    expect({ exitCode: again.exitCode, output: again.output }).toMatchObject({
      exitCode: 0,
    });
    expect(again.stdout).toContain(
      `step 3/6: replace the previous gate root ${first.root}`,
    );
    expect(existsSync(leftover)).toBe(false);
    expect(git(join(first.root, "repo-a"), ["status", "--porcelain"])).toBe(
      " M src/scale.js\n",
    );
  });

  test("refuses a non-empty directory that is not a gate root and leaves it alone", () => {
    const root = newRoot();
    mkdirSync(root);
    writeFileSync(join(root, "precious.txt"), "keep me");
    for (const args of [["--dry-run"], []]) {
      const r = runPrepare(args, { root });
      expect(r.exitCode).toBe(1);
      expect(r.stderr).toContain("is not a wake-gate root");
    }
    expect(readFileSync(join(root, "precious.txt"), "utf8")).toBe("keep me");
  });

  test("an existing empty directory is used as the gate root", () => {
    const root = newRoot();
    mkdirSync(root);
    const r = runPrepare([], { root });
    expect({ exitCode: r.exitCode, output: r.output }).toMatchObject({
      exitCode: 0,
    });
    expect(existsSync(join(root, "repo-a", ".git"))).toBe(true);
  });

  test("refuses to replace a gate root that a process still uses", async () => {
    const first = runPrepare([]);
    expect(first.exitCode).toBe(0);
    const db = join(first.root, "data", "polygloss.db");
    writeFileSync(db, "");
    const holder = Bun.spawn(["bash", "-c", `exec 3<"$1"; sleep 60`, "_", db], {
      env: sandbox.env,
    });
    try {
      // Wait until the holder has the file open.
      await Bun.sleep(300);
      const r = runPrepare([], { root: first.root });
      expect(r.exitCode).toBe(1);
      expect(r.stderr).toContain("still in use");
      expect(r.stderr).toContain(String(holder.pid));
      expect(existsSync(db)).toBe(true);
    } finally {
      holder.kill();
      await holder.exited;
    }
  }, 30_000);

  test("refuses a gate root inside the real data locations", () => {
    for (const root of [
      join(sandbox.home, "Library", "Application Support", "polygloss"),
      join(sandbox.home, ".config", "polygloss-gate"),
      join(sandbox.home, ".claude", "gate"),
      sandbox.home,
      "/",
    ]) {
      for (const args of [["--dry-run"], []]) {
        const r = runPrepare(args, { root });
        expect({ root, exitCode: r.exitCode }).toEqual({ root, exitCode: 1 });
        expect(r.stderr).toContain("refusing");
        expect(r.cargoLog).toEqual([]);
      }
    }
    for (const dir of ["Library", ".claude"])
      expect(existsSync(join(sandbox.home, dir))).toBe(false);
    expect(existsSync(join(sandbox.home, ".config", "polygloss-gate"))).toBe(
      false,
    );
  });

  test("the root guard normalizes .. and compares case-insensitively", () => {
    // APFS is case-insensitive, and `..` can climb back into a real location:
    // neither may slip past the guard (T5.9).
    mkdirSync(join(sandbox.home, "elsewhere"), { recursive: true });
    const upper = sandbox.home.replace(/\/home$/, "/HOME");
    for (const root of [
      join(sandbox.home, "elsewhere") + "/../Library/Application Support/x",
      `${sandbox.home}/./.claude/../.claude/gate`,
      join(sandbox.home, "LIBRARY", "gate"),
      join(sandbox.home, ".CONFIG", "gate"),
      `${upper}/Library/gate`,
      `${sandbox.home}/elsewhere/..`,
    ]) {
      const r = runPrepare(["--dry-run"], { root });
      expect({ root, exitCode: r.exitCode }).toEqual({ root, exitCode: 1 });
      expect(r.stderr).toContain("refusing");
    }
    // A dedicated directory next to them is fine.
    const ok = runPrepare(["--dry-run"], {
      root: join(sandbox.home, "elsewhere", "..", "gate-normalized"),
    });
    expect(ok.exitCode).toBe(0);
    expect(ok.stdout).toContain(join(sandbox.home, "gate-normalized", "data"));
  });

  test("--no-build skips cargo and fails clearly when a binary is missing", () => {
    const missing = runPrepare(["--no-build"], {
      env: { CARGO_TARGET_DIR: join(sandbox.home, "no-target") },
    });
    expect(missing.exitCode).toBe(1);
    expect(missing.stderr).toContain("no-target/debug/Polygloss");
    expect(missing.cargoLog).toEqual([]);
    expect(existsSync(missing.root)).toBe(false);

    const built = runPrepare([]);
    expect(built.exitCode).toBe(0);
    const again = runPrepare(["--no-build"]);
    expect({ exitCode: again.exitCode, output: again.output }).toMatchObject({
      exitCode: 0,
    });
    expect(again.cargoLog).toEqual([]);
    expect(again.stdout).toContain("step 1/6: skip building the app");
  });

  test("a relative --root resolves against the caller's directory", () => {
    const r = Bun.spawnSync([prepare, "--dry-run", "--root", "rel-gate"], {
      cwd: sandbox.home,
      env: {
        ...sandbox.env,
        CARGO_HOME: cargoHome,
        CARGO_TARGET_DIR: join(sandbox.home, "target"),
      },
    });
    expect(r.exitCode).toBe(0);
    expect(r.stdout.toString()).toContain(
      `export POLYGLOSS_DATA_DIR=${join(sandbox.home, "rel-gate", "data")}`,
    );
  });
});

// A dress rehearsal of W1 with the real CLI and plugin hook, the human played
// by the hidden `debug` commands (POLYGLOSS_TEST=1): an assigned review of the
// prepared scratch repo, the Stop hook's exact command, then Submit.
describe("the prepared kit", () => {
  test("rehearses W1 without Claude: the hook command wakes on submit", async () => {
    const r = runPrepare([], {
      env: {
        FAKE_REAL_CLI: cliBin(),
        CARGO_TARGET_DIR: join(sandbox.home, "target-real-cli"),
      },
    });
    expect({ exitCode: r.exitCode, output: r.output }).toMatchObject({
      exitCode: 0,
    });
    const data = join(r.root, "data");
    const env = {
      ...sandbox.env,
      POLYGLOSS_DATA_DIR: data,
      POLYGLOSS_TEST: "1",
      POLYGLOSS_WAIT_OWNER_PID: "0",
    };
    const cli = join(data, "bin", "polygloss");
    const debug = (args: string[]): Record<string, unknown> => {
      const d = Bun.spawnSync([cli, "debug", ...args], { env });
      if (d.exitCode !== 0) throw new Error(d.stderr.toString());
      return JSON.parse(d.stdout.toString());
    };
    const session = "wake-gate-rehearsal";
    const seeded = debug([
      "seed",
      "--repo",
      join(r.root, "repo-a"),
      "--since",
      "HEAD",
    ]);
    const reviewId = seeded.review_id as string;
    debug([
      "assign",
      "--review",
      reviewId,
      "--session",
      session,
      "--client",
      "claude-code",
    ]);

    const hooks = JSON.parse(
      readFileSync(join(pluginDir, "hooks", "hooks.json"), "utf8"),
    );
    const command = hooks.hooks.Stop[0].hooks[0].command as string;
    const hook = Bun.spawn(["sh", "-c", command], {
      env: {
        ...env,
        CLAUDE_PLUGIN_ROOT: pluginDir,
        CLAUDE_CODE_SESSION_ID: session,
      },
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
    });
    try {
      // The waiter registers itself before the human submits.
      const deadline = Date.now() + 10_000;
      let registered = false;
      while (!registered && Date.now() < deadline) {
        await Bun.sleep(100);
        const conn = new Database(join(data, "polygloss.db"), {
          readonly: true,
        });
        try {
          registered =
            conn
              .query("select pid from waiters where session_id = ?")
              .get(session) !== null;
        } finally {
          conn.close();
        }
      }
      expect(registered).toBe(true);

      debug([
        "human-comment",
        "--review",
        reviewId,
        "--path",
        "src/scale.js",
        "--line",
        "7",
        "--body",
        "Call it y.",
      ]);
      debug([
        "human-submit",
        "--review",
        reviewId,
        "--verdict",
        "request-changes",
        "--summary",
        "Rename x to y.",
      ]);
      const exitCode = await hook.exited;
      const stderr = await new Response(hook.stderr).text();
      const stdout = await new Response(hook.stdout).text();
      expect({ exitCode, stdout }).toEqual({ exitCode: 2, stdout: "" });
      expect(stderr).toContain("Verdict: request changes");
      expect(stderr).toContain("Open threads: 1");
      expect(stderr).toContain(`list_threads(review_id="${reviewId}")`);
    } finally {
      hook.kill();
    }
  }, 60_000);
});

// The printed plugin commands, run in a sandboxed Claude Code (never the real
// config): install at local scope into scratch repo A, then clean up.
const claudeCli = Bun.which("claude");
describe.skipIf(claudeCli === null)("the printed plugin commands", () => {
  test("install the plugin at local scope in repo A and remove it again", () => {
    const r = runPrepare([]);
    expect(r.exitCode).toBe(0);
    const lines = r.stdout.split("\n").map((l) => l.trim());
    const pick = (prefix: string): string => {
      const line = lines.find((l) => l.startsWith(prefix));
      if (line === undefined) throw new Error(`no line "${prefix}…"`);
      return line;
    };
    const claudeHome = join(sandbox.home, "claude-home");
    mkdirSync(claudeHome, { recursive: true });
    const repoA = join(r.root, "repo-a");
    const sh = (line: string): { exitCode: number; output: string } => {
      const p = Bun.spawnSync(["bash", "-c", line], {
        cwd: repoA,
        env: {
          ...sandbox.env,
          HOME: claudeHome,
          CLAUDE_CONFIG_DIR: join(claudeHome, ".claude"),
          DISABLE_AUTOUPDATER: "1",
          DISABLE_TELEMETRY: "1",
          DISABLE_ERROR_REPORTING: "1",
          CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
        },
        timeout: 60_000,
      });
      return {
        exitCode: p.exitCode ?? -1,
        output: p.stdout.toString() + p.stderr.toString(),
      };
    };
    for (const prefix of [
      "claude plugin marketplace add ",
      "claude plugin install ",
    ])
      expect(sh(pick(prefix))).toMatchObject({ exitCode: 0 });
    const listed = sh("claude plugin list --json");
    expect(listed.exitCode).toBe(0);
    const plugins = JSON.parse(listed.output) as {
      id: string;
      scope: string;
      enabled: boolean;
      projectPath?: string;
    }[];
    expect(plugins).toEqual([
      expect.objectContaining({
        id: "polygloss@polygloss",
        scope: "local",
        enabled: true,
        projectPath: realpathSync(repoA),
      }),
    ]);
    // The scratch repo's review never shows the plugin settings.
    expect(git(repoA, ["status", "--porcelain"])).toBe(" M src/scale.js\n");

    for (const prefix of [
      "claude plugin uninstall ",
      "claude plugin marketplace remove ",
    ])
      expect(sh(pick(prefix))).toMatchObject({ exitCode: 0 });
    expect(sh("claude plugin list --json").output.trim()).toBe("[]");
  }, 120_000);
});

describe("docs/testing/agent-wake-gate.md", () => {
  const doc = existsSync(gateDoc) ? readFileSync(gateDoc, "utf8") : "";
  const plan = readFileSync(join(repoRoot, "docs", "plan.md"), "utf8");
  const cases = ["W1", "W2", "W3", "W4", "W5", "W6", "W7", "W8"] as const;

  /** The `### <id> …` section, heading included, up to the next heading. */
  function section(id: string): string {
    const lines = doc.split("\n");
    const start = lines.findIndex((l) => l.startsWith(`### ${id} `));
    if (start < 0) return "";
    const end = lines.findIndex((l, i) => i > start && /^#{2,3} /.test(l));
    return lines.slice(start, end < 0 ? undefined : end).join("\n");
  }

  test("every case has its commands, the pass condition and the plan's case name", () => {
    // The case names in the plan's Manual gate table.
    const planNames = new Map<string, string>();
    for (const m of plan.matchAll(
      /^\| (W\d)\s+\| ([^|]+?)\s+\|[^|]+\|[^|]+\|$/gm,
    ))
      planNames.set(m[1] as string, m[2] as string);
    expect([...planNames.keys()]).toEqual([...cases]);
    for (const id of cases) {
      const body = section(id);
      expect({ id, found: body !== "" }).toEqual({ id, found: true });
      expect(body.split("\n")[0]).toBe(`### ${id} ${planNames.get(id)}`);
      expect(body).toContain("```");
      expect(body).toContain("**Pass when:**");
    }
  });

  test("the results table has one empty row per case", () => {
    const header =
      "| Case | Date | Claude Code version | macOS version | Result | Wake latency | Notes |";
    expect(doc).toContain(header);
    const results = doc.slice(doc.indexOf("\n## Results\n"));
    const rows = results
      .slice(0, results.indexOf("\n## ", 1))
      .split("\n")
      .filter((l) => /^\| W\d /.test(l))
      .map((l) =>
        l
          .split("|")
          .slice(1, -1)
          .map((c) => c.trim()),
      );
    expect(rows.map((cells) => cells[0])).toEqual([...cases]);
    // The agent never records a result; the user does.
    for (const cells of rows) expect(cells.slice(1).join("")).toBe("");
  });

  test("says POLYGLOSS_TEST=1 enables the test-only surfaces and goes with POLYGLOSS_APP_BIN", () => {
    const flat = doc.replace(/\s+/g, " ");
    for (const s of [
      "`debug_state`",
      "`polygloss debug …`",
      "W8 runs without",
      "export `POLYGLOSS_TEST=1` and `POLYGLOSS_APP_BIN` together",
    ])
      expect(flat).toContain(s);
  });

  test("the setup commands match what prepare.sh prints", () => {
    const r = runPrepare(["--dry-run"]);
    expect(r.exitCode).toBe(0);
    // The doc writes the default root and `<checkout>` for this checkout.
    const commands = r.stdout
      .split("\n")
      .map((l) => l.trim())
      .filter((l) =>
        /^(export |cd |claude |pgrep |sqlite3 |polygloss )/.test(l),
      )
      .map((l) =>
        l
          .replaceAll(r.targetDir, "<checkout>/target")
          .replaceAll(r.root, "/tmp/polygloss-wake-gate")
          .replaceAll(repoRoot, "<checkout>"),
      );
    expect(commands.length).toBeGreaterThan(10);
    for (const c of commands)
      expect({ c, inDoc: doc.includes(c) }).toEqual({ c, inDoc: true });
  });
});
