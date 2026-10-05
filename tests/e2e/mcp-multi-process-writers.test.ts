// Review focus RF5 (plan T4.11, design §7.1): several processes at once
// against a store that does not exist yet. The app, two `polygloss mcp`
// servers, a `polygloss wait` and the JSON CLI all open the missing
// database together (exactly one migrates), then the two servers and the
// JSON CLI write 200 replies concurrently while the app's feed, the waiter
// and a `wait_for_review` read. No write may fail (no SQLITE_BUSY), and each
// reader sees every event exactly once.
import { Database } from "bun:sqlite";
import {
  afterAll,
  beforeAll,
  describe,
  expect,
  setDefaultTimeout,
  test,
} from "bun:test";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import {
  type AppWorld,
  appWorld,
  debugState,
  git,
  startApp,
  stopAllApps,
  waitForState,
  waitUntil,
} from "../support/app";
import { cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";

setDefaultTimeout(300_000);

type Json = Record<string, any>;
type Env = Record<string, string>;

const REPLIES = { mcpA: 70, mcpB: 70, cli: 60 };
const THREADS = 4;
const CLI_PARALLEL = 6;
// What a store failure under contention would print.
const BUSY = /SQLITE_BUSY|database is locked|database table is locked/i;

/** Runs `polygloss-cli args` without blocking the event loop. */
async function run(
  env: Env,
  args: string[],
  stdin?: string,
): Promise<{ exitCode: number; stdout: string; stderr: string }> {
  const proc = Bun.spawn([cliBin(), ...args], {
    env,
    stdin: stdin === undefined ? "ignore" : new TextEncoder().encode(stdin),
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
  ]);
  return { exitCode: await proc.exited, stdout, stderr };
}

function sql<T>(dataDir: string, query: string, ...params: any[]): T {
  const db = new Database(join(dataDir, "polygloss.db"), { readonly: true });
  try {
    return db.query(query).get(...params) as T;
  } finally {
    db.close();
  }
}

/** The app's log files, joined. With POLYGLOSS_DATA_DIR set (the sandbox),
 * the logs dir is `<data dir>/logs`, never the real ~/Library/Logs. */
function appLog(dataDir: string): string {
  const dir = join(dataDir, "logs");
  if (!existsSync(dir)) return "";
  return readdirSync(dir)
    .map((f) => readFileSync(join(dir, f), "utf8"))
    .join("");
}

/** Runs `tasks` with at most `limit` in flight. */
async function limited<T>(
  tasks: (() => Promise<T>)[],
  limit: number,
): Promise<T[]> {
  const results: T[] = new Array(tasks.length);
  let next = 0;
  const worker = async () => {
    while (next < tasks.length) {
      const i = next++;
      results[i] = await tasks[i]!();
    }
  };
  await Promise.all(Array.from({ length: limit }, worker));
  return results;
}

describe.skipIf(!process.env.POLYGLOSS_E2E)("RF5 multi-process writers", () => {
  // Made in beforeAll: a skipped describe runs its body but no hooks.
  let world: AppWorld;
  beforeAll(() => {
    world = appWorld();
  });
  afterAll(async () => {
    await stopAllApps(world.socket);
    world.cleanup();
  });

  test("first launch, two mcp servers, a waiter and the JSON CLI share one store", async () => {
    const repo = join(world.home, "rf5-repo");
    mkdirSync(repo, { recursive: true });
    git(world.env, repo, ["init", "-q", "-b", "main"]);
    const lines = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`);
    writeFileSync(join(repo, "a.txt"), `${lines.join("\n")}\n`);
    git(world.env, repo, ["add", "."]);
    git(world.env, repo, ["commit", "-q", "-m", "init"]);
    writeFileSync(
      join(repo, "a.txt"),
      `${lines.map((l) => l.toUpperCase()).join("\n")}\n`,
    );
    const repoPath = realpathSync(repo);
    const dbPath = join(world.dataDir, "polygloss.db");
    expect(existsSync(dbPath)).toBe(false);

    // Store bootstrap logs "store migrated before=N after=2" at debug.
    const storeDebug = { RUST_LOG: "warn,polygloss_core::store=debug" };
    const envA = {
      ...world.env,
      ...storeDebug,
      CLAUDE_CODE_SESSION_ID: "rf5-agent-a",
    };
    const envB = {
      ...world.env,
      ...storeDebug,
      CLAUDE_CODE_SESSION_ID: "rf5-agent-b",
    };
    const cliEnv = {
      ...world.env,
      POLYGLOSS_AGENT: "cli-agent",
      CLAUDE_CODE_SESSION_ID: "rf5-cli",
    };
    // Waiters spawned by this runner have it as their parent: no owner pid,
    // so the waiter's session never links to the servers' (OQ-P4).
    const waitEnv = { ...world.env, POLYGLOSS_WAIT_OWNER_PID: "0" };

    // Phase 1: everyone opens the missing store at once.
    const [app, mcpA, mcpB, firstWait, firstList] = await Promise.all([
      startApp(world, {
        env: { POLYGLOSS_LOG: "warn,polygloss_core::store=debug" },
      }),
      connectMcp({ env: envA, clientName: "claude-code", viaShell: true }),
      connectMcp({ env: envB, clientName: "codex", viaShell: true }),
      run(waitEnv, ["wait", "--session", "rf5-agent-a", "--timeout", "60"]),
      run(cliEnv, ["reviews"]),
    ]);
    try {
      // Nothing is assigned yet, so the waiter exits 0 at once.
      expect(firstWait.stderr).toBe("");
      expect(firstWait.exitCode).toBe(0);
      expect(firstList.stderr).toBe("");
      expect(firstList.exitCode).toBe(0);
      expect(JSON.parse(firstList.stdout).reviews).toEqual([]);

      // Exactly one process migrated; nobody backed up or re-migrated.
      expect(
        sql<{ user_version: number }>(world.dataDir, "PRAGMA user_version")
          .user_version,
      ).toBe(2);
      expect(
        readdirSync(world.dataDir).filter((f) => f.includes(".bak-")),
      ).toEqual([]);
      const logs = () => [appLog(world.dataDir), mcpA.stderr(), mcpB.stderr()];
      await waitUntil("every logging process to bootstrap the store", () =>
        logs().every((l) => l.includes("store migrated")),
      );
      const firsts = logs()
        .join("\n")
        .split("\n")
        .filter((l) => /store migrated.*before=0/.test(l));
      expect(firsts.length).toBeLessThanOrEqual(1);

      // The feed is open (it starts at the latest event) and has read
      // everything so far: a poll after the store went quiet.
      const polled = await waitForState(world.socket, (s) =>
        s.feed_polls > 0 ? s : null,
      );
      const base = await waitForState(world.socket, (s) =>
        s.feed_polls > polled.feed_polls ? s : null,
      );
      const seqBase = sql<{ seq: number }>(
        world.dataDir,
        "SELECT COALESCE(MAX(seq), 0) AS seq FROM events",
      ).seq;

      // Phase 2: the agent opens a review (assigned to session A) and
      // leaves threads to reply to.
      const call = async (
        mcp: typeof mcpA,
        name: string,
        args: Json,
      ): Promise<Json> => {
        const res = await mcp.client.callTool(
          { name, arguments: args },
          { timeout: 240_000 },
        );
        if (res.isError)
          throw new Error(`${name}: ${JSON.stringify(res.structuredContent)}`);
        return res.structuredContent as Json;
      };
      const opened = await call(mcpA, "open_diff", {
        repo: repoPath,
        show: true,
      });
      expect(opened.app).toBe("opened");
      const threads: string[] = [];
      for (let i = 0; i < THREADS; i++) {
        const t = await call(mcpA, "create_comment", {
          review_id: opened.review_id,
          kind: "note",
          body_md: `Note ${i + 1}`,
          anchor: { path: "a.txt", line: i + 1 },
        });
        threads.push(t.thread_id);
      }

      // Readers: the Stop-hook waiter of session A and a wait_for_review.
      const waiter = Bun.spawn(
        [cliBin(), "wait", "--session", "rf5-agent-a", "--timeout", "240"],
        { env: waitEnv, stdin: "ignore", stdout: "pipe", stderr: "pipe" },
      );
      const until = Date.now() + 20_000;
      while (
        sql<{ n: number }>(
          world.dataDir,
          "SELECT COUNT(*) AS n FROM waiters WHERE pid = ?",
          waiter.pid,
        ).n === 0
      ) {
        if (Date.now() > until || waiter.exitCode !== null)
          throw new Error(
            `the waiter never registered: ${await new Response(waiter.stderr).text()}`,
          );
        await Bun.sleep(50);
      }
      const since = sql<{ seq: number }>(
        world.dataDir,
        "SELECT MAX(seq) AS seq FROM events",
      ).seq;
      const waitedFor = call(mcpB, "wait_for_review", {
        review_id: opened.review_id,
        since,
        timeout_s: 240,
      });

      // 200 replies at once: two MCP servers and the JSON CLI.
      const thread = (i: number) => threads[i % THREADS]!;
      const viaMcp = (mcp: typeof mcpA, who: string, n: number) =>
        Array.from({ length: n }, (_, i) =>
          call(mcp, "reply", {
            thread_id: thread(i),
            body_md: `${who} reply ${i}`,
          }),
        );
      const viaCli = Array.from(
        { length: REPLIES.cli },
        (_, i) => () =>
          run(
            cliEnv,
            ["reply", thread(i), "--body-file", "-"],
            `cli reply ${i}\n`,
          ),
      );
      const [fromA, fromB, fromCli] = await Promise.all([
        Promise.all(viaMcp(mcpA, "mcp-a", REPLIES.mcpA)),
        Promise.all(viaMcp(mcpB, "mcp-b", REPLIES.mcpB)),
        limited(viaCli, CLI_PARALLEL),
      ]);
      for (const r of fromCli) {
        expect(r.stderr).toBe("");
        expect(r.exitCode).toBe(0);
      }
      const commentIds = new Set([
        ...fromA.map((r) => r.comment_id),
        ...fromB.map((r) => r.comment_id),
        ...fromCli.map((r) => JSON.parse(r.stdout).comment_id),
      ]);
      const total = REPLIES.mcpA + REPLIES.mcpB + REPLIES.cli;
      expect(commentIds.size).toBe(total);
      expect(
        sql<{ n: number }>(
          world.dataDir,
          "SELECT COUNT(*) AS n FROM comments WHERE body_md LIKE '% reply %'",
        ).n,
      ).toBe(total);
      const authors = sql<{ names: string }>(
        world.dataDir,
        "SELECT GROUP_CONCAT(DISTINCT author_name) AS names FROM comments WHERE body_md LIKE '% reply %'",
      ).names.split(",");
      expect(authors.sort()).toEqual(["claude-code", "cli-agent", "codex"]);
      // Replies alone never wake anyone.
      expect(waiter.exitCode).toBeNull();

      // The human submits: the waiter wakes once, and so does wait_for_review.
      const submitted = await run(world.env, [
        "debug",
        "human-submit",
        "--review",
        opened.review_id,
        "--verdict",
        "comment",
        "--summary",
        "Thanks for all the replies.",
      ]);
      expect(submitted.exitCode).toBe(0);
      const waited = await waitedFor;
      expect(waited.outcome).toBe("submitted");
      expect(waited.submission.summary_md).toBe("Thanks for all the replies.");
      const woke = await Promise.race([
        waiter.exited,
        Bun.sleep(30_000).then(() => "timeout" as const),
      ]);
      const waiterErr = await new Response(waiter.stderr).text();
      expect(woke).toBe(2);
      expect(waiterErr.match(/the human submitted their review/g)?.length).toBe(
        1,
      );
      // The wake was consumed: a new waiter has nothing to report.
      const again = await run(waitEnv, [
        "wait",
        "--session",
        "rf5-agent-a",
        "--timeout",
        "1",
      ]);
      expect(again.exitCode).toBe(0);
      expect(again.stderr).toBe("");

      // The app's feed handed out every event after the base exactly once.
      const written = () =>
        sql<{ n: number }>(
          world.dataDir,
          "SELECT COUNT(*) AS n FROM events WHERE seq > ?",
          seqBase,
        ).n;
      const expected = written();
      expect(expected).toBeGreaterThan(total);
      const done = await waitForState(world.socket, (s) =>
        s.events_seen - base.events_seen >= expected ? s : null,
      );
      // A poll after the last write, so any duplicate would show.
      const settled = await waitForState(world.socket, (s) =>
        s.feed_polls > done.feed_polls ? s : null,
      );
      expect(settled.events_seen - base.events_seen).toBe(written());
      expect(settled.feed_errors).toBe(0);
      // The review's tab reloaded its threads and shows the agents' replies.
      expect(
        settled.banners.some(
          (b) => b.review_id === opened.review_id && b.kind === "agent_replies",
        ),
      ).toBe(true);

      // Nothing anywhere reported a busy or locked store.
      for (const text of [
        appLog(world.dataDir),
        app.output(),
        mcpA.stderr(),
        mcpB.stderr(),
        waiterErr,
        ...fromCli.map((r) => r.stderr),
      ])
        expect(text).not.toMatch(BUSY);
    } finally {
      await Promise.all([mcpA.close(), mcpB.close()]);
    }
  });
});
