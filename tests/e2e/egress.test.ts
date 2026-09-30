// Runtime egress audit (plan T5.7, design §19, ADR-0005): while an agent
// drives the real app through `polygloss mcp` (lazy launch, opening a live
// review, comments, replies, resolve, focus, a human submission, a
// re-review request, the long-poll and a worktree edit the watcher sees),
// `lsof -i` never shows an inet socket for either process. The static half
// is scripts/check-deps.sh. Sparkle, the one allowed egress (OQ-16), is off:
// a bundle built without an appcast has no updater, and one built with an
// appcast only loads it idle under POLYGLOSS_TEST=1 (T5.3). With
// POLYGLOSS_BUNDLE_E2E=1, scripts/test-e2e.sh runs this suite again with the
// release bundle's executables.
import {
  afterAll,
  beforeAll,
  describe,
  expect,
  setDefaultTimeout,
  test,
} from "bun:test";
import { mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  type AppWorld,
  appWorld,
  debugState,
  git,
  stopAllApps,
  waitForApp,
  waitForState,
} from "../support/app";
import { cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";

setDefaultTimeout(180_000);

type Json = Record<string, any>;

const LSOF = "/usr/sbin/lsof";

/**
 * The inet (IPv4/IPv6, TCP/UDP) sockets `pids` hold, as `lsof` lines. `-n
 * -P` keep lsof itself from resolving names. lsof exits 1 with no output when
 * nothing matches; anything else unexpected fails the test.
 */
async function inetSockets(pids: number[]): Promise<string[]> {
  const proc = Bun.spawn([LSOF, "-a", "-n", "-P", "-i", "-p", pids.join(",")], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  const lines = stdout.split("\n").filter((l) => l.trim() !== "");
  if (code === 1 && lines.length === 0) return [];
  if (code !== 0)
    throw new Error(`lsof failed (${code}): ${stderr.trim() || stdout}`);
  return lines.slice(1); // the COMMAND/PID/… header
}

/** The unix-domain sockets `pid` holds (`lsof -U`), for the probe's control. */
function unixSockets(pid: number): string[] {
  const r = Bun.spawnSync([LSOF, "-a", "-n", "-P", "-U", "-p", String(pid)]);
  return r.stdout
    .toString()
    .split("\n")
    .slice(1)
    .filter((l) => l.trim() !== "");
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/**
 * Samples `inetSockets(pids)` back to back until stopped; returns every
 * socket line seen and the number of samples.
 */
function sampler(pids: number[]): {
  stop: () => Promise<{ seen: string[]; samples: number }>;
} {
  let running = true;
  let samples = 0;
  const seen: string[] = [];
  const loop = (async () => {
    while (running) {
      seen.push(...(await inetSockets(pids)));
      samples += 1;
    }
  })();
  // Stopping twice (the test's `finally`) is harmless.
  return {
    stop: async () => {
      running = false;
      await loop;
      return { seen, samples };
    },
  };
}

describe.skipIf(!process.env.POLYGLOSS_E2E)("egress audit", () => {
  let world: AppWorld;
  beforeAll(() => {
    world = appWorld({ session: "egress-agent" });
  });

  afterAll(async () => {
    await stopAllApps(world.socket);
    world.cleanup();
  });

  test("the probe sees an inet socket when a process holds one", async () => {
    const server = Bun.listen({
      hostname: "127.0.0.1",
      port: 0,
      socket: { data() {} },
    });
    try {
      const lines = await inetSockets([process.pid]);
      expect(lines.some((l) => l.includes(`127.0.0.1:${server.port}`))).toBe(
        true,
      );
    } finally {
      server.stop(true);
    }
  });

  test("app and mcp open no inet sockets during e2e", async () => {
    const env = world.env;
    const repo = join(world.home, "repo");
    mkdirSync(repo);
    git(env, repo, ["init", "-q", "-b", "main"]);
    const lines = Array.from({ length: 40 }, (_, i) => `line ${i + 1}`);
    writeFileSync(join(repo, "a.txt"), `${lines.join("\n")}\n`);
    git(env, repo, ["add", "."]);
    git(env, repo, ["commit", "-q", "-m", "init"]);
    writeFileSync(
      join(repo, "a.txt"),
      `${lines.map((l, i) => (i === 4 ? "LINE 5" : l)).join("\n")}\n`,
    );

    const mcp = await connectMcp({ env, clientName: "claude-code" });
    let sampling: ReturnType<typeof sampler> | undefined;
    try {
      const call = async (name: string, args: Json = {}): Promise<Json> => {
        const res = await mcp.client.callTool({ name, arguments: args });
        if (res.isError)
          throw new Error(
            `${name} failed: ${JSON.stringify(res.structuredContent)}`,
          );
        return res.structuredContent as Json;
      };
      const mcpPid = mcp.pid()!;
      expect(mcpPid).toBeGreaterThan(0);

      // The agent's first open_diff launches the app (POLYGLOSS_APP_BIN).
      const opened = await call("open_diff", {
        repo: realpathSync(repo),
        show: true,
      });
      // Registered first, so afterAll stops it even if a check below fails.
      const appPid = await waitForApp(world.socket);
      expect(opened.app).toBe("launched");
      const pids = [appPid, mcpPid];
      sampling = sampler(pids);

      await call("list_reviews");
      const note = await call("create_comment", {
        review_id: opened.review_id,
        kind: "note",
        body_md: "Upper-cased on purpose.",
        anchor: { path: "a.txt", side: "new", line: 5 },
      });
      await call("get_thread", { thread_id: note.thread_id });
      await call("focus", {
        review_id: opened.review_id,
        path: "a.txt",
        line: 30,
      });
      // The human comments and submits; the agent replies and resolves.
      const debug = (args: string[]): Json => {
        const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
        if (r.exitCode !== 0)
          throw new Error(`debug ${args[0]}: ${r.stderr.toString()}`);
        return JSON.parse(r.stdout.toString()) as Json;
      };
      const human = debug([
        "human-comment",
        "--review",
        opened.review_id,
        "--path",
        "a.txt",
        "--line",
        "5",
        "--body",
        "Why upper case?",
      ]);
      debug([
        "human-submit",
        "--review",
        opened.review_id,
        "--verdict",
        "request-changes",
      ]);
      await call("list_threads", { review_id: opened.review_id });
      await call("wait_for_review", {
        review_id: opened.review_id,
        timeout_s: 1,
      });
      await call("reply", {
        thread_id: human.thread_id,
        body_md: "It matches the constant.",
        resolve: true,
      });
      await call("unresolve", { thread_id: human.thread_id });
      await call("request_rereview", {
        review_id: opened.review_id,
        summary_md: "Explained the upper case.",
      });
      await waitForState(world.socket, (s) =>
        s.banners.some(
          (b) => b.review_id === opened.review_id && b.kind === "rereview",
        ),
      );
      // A worktree edit: the live watcher and the snapshot machinery run.
      writeFileSync(join(repo, "b.txt"), "new file\n");
      await Bun.sleep(2_000);
      await debugState(world.socket);

      const { seen, samples } = await sampling.stop();
      // Both processes lived through the whole run (lsof reports nothing
      // for a pid that is gone), and lsof could see into them.
      expect(pids.map(alive)).toEqual([true, true]);
      expect(
        unixSockets(appPid).some((l) => l.includes("polygloss.sock")),
      ).toBe(true);
      expect(samples).toBeGreaterThanOrEqual(5);
      expect(seen).toEqual([]);
    } finally {
      await sampling?.stop();
      await mcp.close();
    }
  });
});
