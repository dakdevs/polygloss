// Agent surface against the real running app (plan T4.11, design §13, §15,
// §17): `polygloss mcp` tools drive a `Polygloss` started in a sandbox with
// POLYGLOSS_TEST=1, and the test reads the app's state back through the
// test-only `debug_state` socket op. The human is played by the hidden
// `polygloss-cli debug` commands. Windows appear on screen while this runs.
import { Database } from "bun:sqlite";
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
  appCall,
  appWorld,
  debugState,
  git,
  startApp,
  stopAllApps,
  waitForApp,
  waitForState,
} from "../support/app";
import { cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";

// App startup, live snapshots and the feed's polls in a debug build.
setDefaultTimeout(120_000);

type Json = Record<string, any>;
type Env = Record<string, string>;

let repoCount = 0;

/**
 * A repo on `main` with `a.txt` (ten lines) and `big.txt` (500 lines)
 * committed; the worktree edits `a.txt` line 5 (uniquely per repo) and every
 * line of `big.txt`,
 * so both files show in a live review and `big.txt` is a single long hunk.
 */
function makeRepo(env: Env, home: string): string {
  repoCount += 1;
  const repo = join(home, `repo-${repoCount}`);
  mkdirSync(repo, { recursive: true });
  git(env, repo, ["init", "-q", "-b", "main"]);
  const ten = Array.from({ length: 10 }, (_, i) => `l${i + 1}`);
  const big = Array.from({ length: 500 }, (_, i) => `line ${i + 1}`);
  const write = (file: string, lines: string[]) =>
    writeFileSync(join(repo, file), `${lines.join("\n")}\n`);
  write("a.txt", ten);
  write("big.txt", big);
  git(env, repo, ["add", "."]);
  git(env, repo, ["commit", "-q", "-m", "init"]);
  write(
    "a.txt",
    // Unique per repo: equal trees would share one content-addressed diff id.
    ten.map((l) => (l === "l5" ? `L5 of repo ${repoCount}` : l)),
  );
  write(
    "big.txt",
    big.map((l) => l.toUpperCase()),
  );
  return realpathSync(repo);
}

function debug(env: Env, args: string[]): Json {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
  if (r.exitCode !== 0)
    throw new Error(`debug ${args.join(" ")}: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString()) as Json;
}

/** Rows of the `waiters` table registered by `pid`. */
function waiterRows(dataDir: string, pid: number): number {
  const db = new Database(join(dataDir, "polygloss.db"), { readonly: true });
  try {
    const row = db
      .query("SELECT COUNT(*) AS n FROM waiters WHERE pid = ?")
      .get(pid) as { n: number };
    return row.n;
  } finally {
    db.close();
  }
}

type Call = (name: string, args?: Json) => Promise<Json>;

/**
 * Runs `f` with a fresh `polygloss mcp` (client `claude-code`, session =
 * `CLAUDE_CODE_SESSION_ID` of `env`). The server runs under its own `sh`,
 * so servers of one test runner never share an owner pid (§16.4).
 */
async function withMcp<T>(env: Env, f: (call: Call) => Promise<T>): Promise<T> {
  const mcp = await connectMcp({
    env,
    clientName: "claude-code",
    viaShell: true,
  });
  try {
    return await f(async (name, args = {}) => {
      const res = await mcp.client.callTool({ name, arguments: args });
      const content = res.structuredContent as Json;
      if (res.isError)
        throw new Error(`${name} failed: ${JSON.stringify(content)}`);
      return content;
    });
  } finally {
    await mcp.close();
  }
}

describe.skipIf(!process.env.POLYGLOSS_E2E)(
  "MCP tools against a running app",
  () => {
    // Made in beforeAll: a skipped describe runs its body but no hooks.
    let world: AppWorld;
    beforeAll(async () => {
      world = appWorld({ session: "e2e-agent" });
      await startApp(world);
    });

    afterAll(async () => {
      await stopAllApps(world.socket);
      world.cleanup();
    });

    /** A live review of a new repo opened by the agent, shown in the app. */
    async function openReview(call: Call): Promise<Json> {
      const repo = makeRepo(world.env, world.home);
      const opened = await call("open_diff", { repo, show: true });
      expect(opened.app).toBe("opened");
      return { ...opened, repo };
    }

    test("open_diff opens a tab in the running app", async () => {
      await withMcp(world.env, async (call) => {
        const before = await debugState(world.socket);
        const opened = await openReview(call);
        // The tab is open by the time open_diff answers.
        const state = await debugState(world.socket);
        expect(state.window_open).toBe(true);
        expect(state.tabs.length).toBe(before.tabs.length + 1);
        const tab = state.tabs.find((t) => t.review_id === opened.review_id);
        expect(tab).toMatchObject({
          review_id: opened.review_id,
          diff_id: opened.diff_id,
          active: true,
        });
        expect(state.focused_tab).toBe(opened.review_id);
        // Agents never bring the app forward (design §13.4, §15.1).
        expect(state.activations).toBe(before.activations);
        // Opening it again focuses the same tab instead of adding one.
        const again = await call("open_diff", {
          repo: opened.repo,
          show: true,
        });
        expect(again.review_id).toBe(opened.review_id);
        expect(again.app).toBe("opened");
        const after = await debugState(world.socket);
        expect(
          after.tabs.filter((t) => t.review_id === opened.review_id).length,
        ).toBe(1);
      });
    });

    test("an MCP-opened review shows its tests in a closed section", async () => {
      await withMcp(world.env, async (call) => {
        // A live review of `src/a.ts` and its test (design §11.15: Tests is
        // on by default and its section starts closed).
        repoCount += 1;
        const repo = join(world.home, `repo-${repoCount}`);
        mkdirSync(join(repo, "src"), { recursive: true });
        git(world.env, repo, ["init", "-q", "-b", "main"]);
        writeFileSync(join(repo, "src/a.ts"), "export const a = 1;\n");
        writeFileSync(join(repo, "src/a.test.ts"), "test('a', () => {});\n");
        git(world.env, repo, ["add", "."]);
        git(world.env, repo, ["commit", "-q", "-m", "init"]);
        writeFileSync(join(repo, "src/a.ts"), "export const a = 2;\n");
        writeFileSync(
          join(repo, "src/a.test.ts"),
          `test('a', () => { /* repo ${repoCount} */ });\n`,
        );
        const opened = await call("open_diff", {
          repo: realpathSync(repo),
          show: true,
        });
        expect(opened.app).toBe("opened");
        const state = await waitForState(world.socket, (s) =>
          s.tabs.find((t) => t.review_id === opened.review_id),
        );
        expect(state.sections).toEqual([
          {
            category: "tests",
            files: ["src/a.test.ts"],
            open: false,
            open_threads: 0,
            agent: false,
            changed_since_viewed: false,
          },
        ]);
        // The view starts on the main file.
        expect(state.anchor).toMatchObject({ path: "src/a.ts", line: 1 });
      });
    });

    test("focus scrolls to path and line", async () => {
      await withMcp(world.env, async (call) => {
        const opened = await openReview(call);
        const shown = await debugState(world.socket);
        const tab = shown.tabs.find((t) => t.review_id === opened.review_id);
        // Freshly opened at the top of the first file.
        expect(tab?.anchor).toMatchObject({ path: "a.txt", line: 1 });

        const focused = await call("focus", {
          review_id: opened.review_id,
          path: "big.txt",
          line: 400,
        });
        expect(focused.status).toBe("focused");
        const state = await waitForState(world.socket, (s) => {
          const t = s.tabs.find((x) => x.review_id === opened.review_id);
          return t?.cursor?.line === 400 ? s : null;
        });
        const after = state.tabs.find((t) => t.review_id === opened.review_id)!;
        expect(after.cursor).toEqual({
          path: "big.txt",
          side: "new",
          line: 400,
        });
        // The viewport scrolled: its top line is now in big.txt, near line 400.
        expect(after.anchor?.path).toBe("big.txt");
        expect(after.anchor!.line).toBeGreaterThan(300);
        expect(after.anchor!.line).toBeLessThanOrEqual(400);
        expect(state.focused_tab).toBe(opened.review_id);

        // Old side of a line, by diff id.
        const old = await call("focus", {
          diff_id: opened.diff_id,
          path: "a.txt",
          side: "old",
          line: 5,
        });
        expect(old.status).toBe("focused");
        await waitForState(world.socket, (s) => {
          const t = s.tabs.find((x) => x.review_id === opened.review_id);
          return (
            t?.cursor?.path === "a.txt" &&
            t.cursor.side === "old" &&
            t.cursor.line === 5
          );
        });
      });
    });

    test("request_rereview shows the banner and badge count", async () => {
      await withMcp(world.env, async (call) => {
        const opened = await openReview(call);
        const before = await debugState(world.socket);
        expect(
          before.banners.filter((b) => b.review_id === opened.review_id),
        ).toEqual([]);

        const rereview = await call("request_rereview", {
          review_id: opened.review_id,
          summary_md: "Renamed the helper and **fixed** the off-by-one.",
        });
        expect(rereview.status).toBe("rereview_requested");
        // The write nudges the running app; the feed shows the banner and the
        // Dock badge counts the review as awaiting the human.
        const state = await waitForState(world.socket, (s) =>
          s.banners.some(
            (b) => b.review_id === opened.review_id && b.kind === "rereview",
          ) && s.badge === before.badge + 1
            ? s
            : null,
        );
        const banner = state.banners.find(
          (b) => b.review_id === opened.review_id && b.kind === "rereview",
        )!;
        expect(banner.text).toContain("claude-code");
        expect(banner.text).toContain(
          "Renamed the helper and fixed the off-by-one.",
        );
        // No new window or tab: the review's tab shows it.
        expect(
          state.tabs.filter((t) => t.review_id === opened.review_id).length,
        ).toBe(1);
      });
    });

    test("agent reply shows the replied banner", async () => {
      await withMcp(world.env, async (call) => {
        const opened = await openReview(call);
        // The human comments and submits (as the app would).
        const comment = debug(world.env, [
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
        debug(world.env, [
          "human-submit",
          "--review",
          opened.review_id,
          "--verdict",
          "request-changes",
          "--summary",
          "One question.",
        ]);
        const quiet = await debugState(world.socket);
        expect(
          quiet.banners.filter(
            (b) =>
              b.review_id === opened.review_id && b.kind === "agent_replies",
          ),
        ).toEqual([]);

        const reply = await call("reply", {
          thread_id: comment.thread_id,
          body_md: "It matches the constant's name.",
        });
        expect(reply.thread_id).toBe(comment.thread_id);
        const state = await waitForState(world.socket, (s) =>
          s.banners.find(
            (b) =>
              b.review_id === opened.review_id && b.kind === "agent_replies",
          ),
        );
        expect(state.text).toBe("claude-code replied to 1 thread");
      });
    });

    test("human submit via debug command wakes polygloss wait", async () => {
      // Its own agent session, so earlier tests' submissions do not wake it.
      const env = { ...world.env, CLAUDE_CODE_SESSION_ID: "e2e-waiting-agent" };
      await withMcp(env, async (call) => {
        const opened = await openReview(call);
        const seenBefore = (await debugState(world.socket)).events_seen;
        const waiter = Bun.spawn(
          [
            cliBin(),
            "wait",
            "--session",
            "e2e-waiting-agent",
            "--timeout",
            "120",
          ],
          {
            // No owner pid: the waiter's parent is this test runner (OQ-P4).
            env: { ...env, POLYGLOSS_WAIT_OWNER_PID: "0" },
            stdin: "ignore",
            stdout: "pipe",
            stderr: "pipe",
          },
        );
        try {
          // Listening once its waiter row exists.
          const until = Date.now() + 20_000;
          while (waiterRows(world.dataDir, waiter.pid) === 0) {
            if (Date.now() > until || waiter.exitCode !== null)
              throw new Error(
                `polygloss wait never registered: ${await new Response(waiter.stderr).text()}`,
              );
            await Bun.sleep(50);
          }

          debug(env, [
            "human-comment",
            "--review",
            opened.review_id,
            "--body",
            "Looks good overall.",
          ]);
          debug(env, [
            "human-submit",
            "--review",
            opened.review_id,
            "--verdict",
            "approve",
            "--summary",
            "Ship it.",
          ]);
          const exitCode = await Promise.race([
            waiter.exited,
            Bun.sleep(20_000).then(() => "timeout" as const),
          ]);
          const stderr = await new Response(waiter.stderr).text();
          expect(exitCode).toBe(2);
          expect(stderr).toContain(
            "Polygloss: the human submitted their review",
          );
          expect(stderr).toContain("Verdict: approve");
          expect(stderr).toContain("Summary: Ship it.");
          expect(stderr).toContain(
            `list_threads(review_id="${opened.review_id}")`,
          );
          expect(await new Response(waiter.stdout).text()).toBe("");
          // The running app read the submission from the store too.
          await waitForState(world.socket, (s) => s.events_seen > seenBefore);
        } finally {
          waiter.kill();
          await waiter.exited;
        }
      });
    });
  },
);

describe.skipIf(!process.env.POLYGLOSS_E2E)("MCP without a running app", () => {
  let world: AppWorld;
  beforeAll(() => {
    world = appWorld({ session: "e2e-lazy-agent" });
  });

  afterAll(async () => {
    await stopAllApps(world.socket);
    world.cleanup();
  });

  test("open_diff launches the app lazily when it is not running", async () => {
    await withMcp(world.env, async (call) => {
      // Startup and reads never launch it.
      await call("list_reviews", {});
      await expect(appCall(world.socket, "hello", {}, 1_000)).rejects.toThrow();

      const repo = makeRepo(world.env, world.home);
      const opened = await call("open_diff", { repo, show: true });
      expect(opened.app).toBe("launched");
      // POLYGLOSS_APP_BIN started this checkout's app; it shows the review.
      const launchedPid = await waitForApp(world.socket);
      const state = await waitForState(world.socket, (s) =>
        s.tabs.some((t) => t.review_id === opened.review_id) ? s : null,
      );
      expect(state.focused_tab).toBe(opened.review_id);
      expect(
        state.tabs.find((t) => t.review_id === opened.review_id),
      ).toMatchObject({ diff_id: opened.diff_id, active: true });
      // A background launch (`open -g`; here the launch override with
      // POLYGLOSS_LAUNCH_ACTIVATE=0) never brings the app forward, not even
      // at startup (design §13.4).
      expect(state.activations).toBe(0);

      // A second open_diff finds it running.
      const again = await call("open_diff", { repo, show: true });
      expect(again.app).toBe("opened");
      expect(await waitForApp(world.socket)).toBe(launchedPid);
    });
  });
});
