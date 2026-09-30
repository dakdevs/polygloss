// `polygloss wait` (T4.8, design §16.3): the Stop-hook waiter. Exit 0 when
// there is nothing to report (no assigned open review, timeout, replaced),
// exit 2 with a summary on stderr when a review assigned to the session is
// submitted, exit 1 on errors. The human's side is played by the hidden
// `debug seed|assign|human-comment|human-submit` commands (POLYGLOSS_TEST=1).
import { Database } from "bun:sqlite";
import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());
const testEnv = { ...sandbox.env, POLYGLOSS_TEST: "1" };
// Every waiter spawned here has this test process as its parent, and waiters
// record their parent as the session's owner pid, which links sessions with
// the same owner (design §16.4). Tests pin "no owner" (test-only override,
// OQ-P4) so their sessions stay apart; the drift test uses the real parent.
const waitEnv = { ...testEnv, POLYGLOSS_WAIT_OWNER_PID: "0" };
// Waiters listen for seconds; bun's default per-test timeout is 5 s.
const SLOW = 30_000;

let repoCount = 0;
let sessionCount = 0;

function git(repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env: sandbox.env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
  return r.stdout.toString();
}

/** A fresh session id for one test. */
function newSession(): string {
  sessionCount += 1;
  return `wait-test-session-${sessionCount}`;
}

/** A repo with one commit and an uncommitted edit of `a.txt`. */
function liveRepo(): string {
  repoCount += 1;
  const repo = join(sandbox.home, `repo-${repoCount}`);
  mkdirSync(repo, { recursive: true });
  git(repo, ["init", "-q", "-b", "main"]);
  writeFileSync(join(repo, "a.txt"), "one\ntwo\nthree\n");
  git(repo, ["add", "a.txt"]);
  git(repo, ["commit", "-q", "-m", "init"]);
  writeFileSync(join(repo, "a.txt"), "one\nTWO\nthree\nfour\n");
  return repo;
}

function debug(args: string[]): Record<string, unknown> {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env: testEnv });
  if (r.exitCode !== 0)
    throw new Error(`debug ${args[0]} failed: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString());
}

/** Seeds a live review and assigns it to `session` (as `open_diff` would). */
function assignedReview(
  session: string,
  extra: string[] = [],
): { reviewId: string } {
  const seeded = debug(["seed", "--repo", liveRepo(), "--since", "HEAD"]);
  const reviewId = seeded.review_id as string;
  debug([
    "assign",
    "--review",
    reviewId,
    "--session",
    session,
    "--client",
    "claude-code",
    ...extra,
  ]);
  return { reviewId };
}

/** A human line comment (a draft until submitted) on `a.txt:2`. */
function humanComment(reviewId: string, body: string): void {
  debug([
    "human-comment",
    "--review",
    reviewId,
    "--path",
    "a.txt",
    "--line",
    "2",
    "--body",
    body,
  ]);
}

function submit(
  reviewId: string,
  summary: string,
  verdict = "request-changes",
): { seq: number } {
  const r = debug([
    "human-submit",
    "--review",
    reviewId,
    "--verdict",
    verdict,
    "--summary",
    summary,
  ]);
  return { seq: r.seq as number };
}

function db(): Database {
  return new Database(join(sandbox.dataDir, "polygloss.db"), {
    readwrite: true,
  });
}

function query<T>(sql: string, ...params: (string | number)[]): T | null {
  const conn = db();
  try {
    return conn.query(sql).get(...params) as T | null;
  } finally {
    conn.close();
  }
}

type Waiter = {
  proc: Bun.Subprocess<"pipe" | "ignore", "pipe", "pipe">;
  done: Promise<{
    exitCode: number | null;
    stderr: string;
    stdout: string;
    ms: number;
  }>;
};

/** Starts `polygloss wait` without waiting for it. */
function startWait(
  args: string[],
  opts: { stdin?: string; env?: Record<string, string> } = {},
): Waiter {
  const started = performance.now();
  const proc = Bun.spawn([cliBin(), "wait", ...args], {
    env: opts.env ?? waitEnv,
    stdin: opts.stdin === undefined ? "ignore" : "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  if (
    opts.stdin !== undefined &&
    proc.stdin &&
    typeof proc.stdin !== "number"
  ) {
    proc.stdin.write(opts.stdin);
    // Like Claude Code, leave stdin open: the waiter must not wait for EOF.
    proc.stdin.flush();
  }
  const done = (async () => {
    const [stdout, stderr] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
    ]);
    const exitCode = await proc.exited;
    return { exitCode, stderr, stdout, ms: performance.now() - started };
  })();
  return { proc, done };
}

/** Runs `polygloss wait` to completion. */
async function runWait(
  args: string[],
  opts: { stdin?: string; env?: Record<string, string> } = {},
): Promise<{ exitCode: number | null; stderr: string; ms: number }> {
  return startWait(args, opts).done;
}

/** Resolves once the waiters table holds `pid` (the waiter is listening). */
async function registered(pid: number, timeoutMs = 10_000): Promise<void> {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    const row = query<{ n: number }>(
      "SELECT COUNT(*) AS n FROM waiters WHERE pid = ?",
      pid,
    );
    if (row && row.n > 0) return;
    await Bun.sleep(50);
  }
  throw new Error(`waiter ${pid} never registered`);
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** Appends an event row the way an agent write would (no human involved). */
function insertEvent(kind: string, reviewId: string, payload: unknown): void {
  const conn = db();
  try {
    conn
      .query(
        "INSERT INTO events (at, kind, review_id, actor_kind, actor_name, payload) VALUES (?, ?, ?, 'agent', 'claude-code', ?)",
      )
      .run(Date.now(), kind, reviewId, JSON.stringify(payload));
  } finally {
    conn.close();
  }
}

describe("polygloss wait", () => {
  test(
    "exits 0 immediately when no review is assigned",
    async () => {
      const r = await runWait(["--session", newSession()]);
      expect(r.exitCode).toBe(0);
      expect(r.ms).toBeLessThan(5_000);
      // A waiter that could never fire is not left registered.
      expect(
        query<{ n: number }>("SELECT COUNT(*) AS n FROM waiters")?.n ?? 0,
      ).toBe(0);
    },
    SLOW,
  );

  test(
    "exits 2 with a summary on stderr after submit",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      humanComment(reviewId, "Why uppercase?");
      const waiter = startWait(["--session", session]);
      await registered(waiter.proc.pid);

      const { seq } = submit(reviewId, "Please explain the rename.");
      const r = await waiter.done;
      expect(r.exitCode).toBe(2);
      expect(r.stdout).toBe("");
      expect(r.stderr).toContain("request changes");
      expect(r.stderr).toContain("Please explain the rename.");
      expect(r.stderr).toContain("Open threads: 1");
      expect(r.stderr).toContain(`list_threads(review_id="${reviewId}")`);

      const row = query<{ last_woken_seq: number }>(
        "SELECT last_woken_seq FROM sessions WHERE id = ?",
        session,
      );
      expect(row?.last_woken_seq).toBe(seq);
      // The waiter removed its own row on the way out.
      expect(
        query<{ n: number }>(
          "SELECT COUNT(*) AS n FROM waiters WHERE pid = ?",
          waiter.proc.pid,
        )?.n,
      ).toBe(0);
    },
    SLOW,
  );

  test(
    "reads session_id from hook stdin",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      submit(reviewId, "From the hook.", "comment");
      const hookInput = (id: string) =>
        JSON.stringify({
          session_id: id,
          transcript_path: "/tmp/transcript.jsonl",
          cwd: sandbox.home,
          hook_event_name: "Stop",
          stop_hook_active: false,
        }) + "\n";

      // Another session's hook input has nothing to report.
      const other = await runWait([], { stdin: hookInput(newSession()) });
      expect(other.exitCode).toBe(0);

      // The plugin hook passes `--session "$CLAUDE_CODE_SESSION_ID"`, which can
      // be empty: stdin's session_id is used then.
      const r = await runWait(["--session", ""], { stdin: hookInput(session) });
      expect(r.exitCode).toBe(2);
      expect(r.stderr).toContain("From the hook.");
      expect(r.stderr).toContain("comment");
      expect(r.ms).toBeLessThan(5_000);
    },
    SLOW,
  );

  test(
    "newer waiter replaces the older one",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      const first = startWait(["--session", session]);
      await registered(first.proc.pid);
      const second = startWait(["--session", session]);
      await registered(second.proc.pid);

      const firstResult = await first.done;
      expect(firstResult.exitCode).toBe(0);
      expect(firstResult.stderr).toBe("");
      const row = query<{ pid: number }>(
        "SELECT pid FROM waiters WHERE session_id = ?",
        session,
      );
      expect(row?.pid).toBe(second.proc.pid);

      submit(reviewId, "Only the newer waiter reports this.");
      const secondResult = await second.done;
      expect(secondResult.exitCode).toBe(2);
      expect(secondResult.stderr).toContain(
        "Only the newer waiter reports this.",
      );
    },
    SLOW,
  );

  test(
    "a replaced waiter that is not polygloss-cli is never signaled",
    async () => {
      const session = newSession();
      assignedReview(session);
      // A stranger process whose pid sits in the session's waiter row.
      const stranger = Bun.spawn(["/bin/sleep", "30"]);
      try {
        const conn = db();
        try {
          conn
            .query(
              "INSERT INTO waiters (session_id, pid, started_at, deadline_at) VALUES (?, ?, ?, ?)",
            )
            .run(session, stranger.pid, Date.now(), Date.now() + 3_600_000);
        } finally {
          conn.close();
        }
        const r = await runWait(["--session", session, "--timeout", "33"]);
        expect(r.exitCode).toBe(0);
        expect(alive(stranger.pid)).toBe(true);
      } finally {
        stranger.kill();
      }
    },
    SLOW,
  );

  test(
    "a stuck older waiter is signaled after the grace period",
    async () => {
      const session = newSession();
      assignedReview(session);
      // A real `polygloss-cli wait` on another store never sees its row replaced
      // here, so only the signal stops it.
      const otherData = join(sandbox.home, "other-data");
      mkdirSync(otherData, { recursive: true, mode: 0o700 });
      const otherEnv = { ...waitEnv, POLYGLOSS_DATA_DIR: otherData };
      const otherSession = "stuck-waiter";
      const otherSeed = Bun.spawnSync(
        [cliBin(), "debug", "seed", "--repo", liveRepo(), "--since", "HEAD"],
        { env: otherEnv },
      );
      const otherReview = JSON.parse(otherSeed.stdout.toString()).review_id;
      Bun.spawnSync(
        [
          cliBin(),
          "debug",
          "assign",
          "--review",
          otherReview,
          "--session",
          otherSession,
        ],
        { env: otherEnv },
      );
      const stuck = startWait(["--session", otherSession], { env: otherEnv });
      await Bun.sleep(500);
      expect(alive(stuck.proc.pid)).toBe(true);

      const conn = db();
      try {
        conn
          .query(
            "INSERT INTO waiters (session_id, pid, started_at, deadline_at) VALUES (?, ?, ?, ?)",
          )
          .run(session, stuck.proc.pid, Date.now(), Date.now() + 3_600_000);
      } finally {
        conn.close();
      }
      const newer = startWait(["--session", session, "--timeout", "36"]);
      const stuckResult = await stuck.done;
      // SIGTERM: the waiter stops listening and exits 0 (never a wake-up),
      // removing its own row on the way out.
      expect(stuckResult.exitCode).toBe(0);
      expect(stuckResult.stderr).toBe("");
      const otherDb = new Database(join(otherData, "polygloss.db"));
      try {
        expect(
          (
            otherDb
              .query("SELECT COUNT(*) AS n FROM waiters WHERE pid = ?")
              .get(stuck.proc.pid) as { n: number }
          ).n,
        ).toBe(0);
      } finally {
        otherDb.close();
      }
      const newerResult = await newer.done;
      expect(newerResult.exitCode).toBe(0);
    },
    SLOW,
  );

  test(
    "a same-named process on a reused pid is never signaled",
    async () => {
      const session = newSession();
      assignedReview(session);
      // The waiter row predates the process that now holds its pid: an
      // executable named polygloss-cli (here a `polygloss-cli mcp` server) is
      // not proof enough that it is the waiter (PID reuse).
      const before = Date.now() - 60_000;
      const stranger = Bun.spawn([cliBin(), "mcp"], {
        env: testEnv,
        stdin: "pipe",
        stdout: "ignore",
        stderr: "ignore",
      });
      try {
        await Bun.sleep(200);
        expect(alive(stranger.pid)).toBe(true);
        const conn = db();
        try {
          conn
            .query(
              "INSERT INTO waiters (session_id, pid, started_at, deadline_at) VALUES (?, ?, ?, ?)",
            )
            .run(session, stranger.pid, before, Date.now() + 3_600_000);
        } finally {
          conn.close();
        }
        const r = await runWait(["--session", session, "--timeout", "34"]);
        expect(r.exitCode).toBe(0);
        expect(alive(stranger.pid)).toBe(true);
        expect(stranger.signalCode).toBeNull();
      } finally {
        stranger.kill();
      }
    },
    SLOW,
  );

  test(
    "SIGTERM ends the wait with exit 0 and removes the waiter row",
    async () => {
      const session = newSession();
      assignedReview(session);
      const waiter = startWait(["--session", session]);
      await registered(waiter.proc.pid);
      waiter.proc.kill("SIGTERM");
      const r = await waiter.done;
      expect(r.exitCode).toBe(0);
      expect(r.stderr).toBe("");
      expect(
        query<{ n: number }>(
          "SELECT COUNT(*) AS n FROM waiters WHERE session_id = ?",
          session,
        )?.n,
      ).toBe(0);
    },
    SLOW,
  );

  test(
    "a submission from before the session's assignment does not wake it",
    async () => {
      // An earlier session had the review; the human approved it then. A new
      // session that opens the same review must not be woken with that.
      const earlier = newSession();
      const { reviewId } = assignedReview(earlier);
      submit(reviewId, "Old approval.", "approve");
      const session = newSession();
      debug([
        "assign",
        "--review",
        reviewId,
        "--session",
        session,
        "--client",
        "claude-code",
      ]);
      const r = await runWait(["--session", session, "--timeout", "31"]);
      expect(r.exitCode).toBe(0);
      expect(r.stderr).toBe("");

      // A submission after the assignment still wakes it.
      submit(reviewId, "New feedback.", "comment");
      const later = await runWait(["--session", session]);
      expect(later.exitCode).toBe(2);
      expect(later.stderr).toContain("New feedback.");
      expect(later.stderr).not.toContain("Old approval.");
    },
    SLOW,
  );

  test(
    "exits 0 thirty seconds before the deadline",
    async () => {
      const session = newSession();
      assignedReview(session);
      const r = await runWait(["--session", session, "--timeout", "32"]);
      expect(r.exitCode).toBe(0);
      expect(r.stderr).toBe("");
      expect(r.ms).toBeGreaterThanOrEqual(1_800);
      expect(r.ms).toBeLessThan(10_000);

      // POLYGLOSS_WAIT_TIMEOUT_S (OQ-P14) is the default when --timeout is absent.
      const env = await runWait(["--session", session], {
        env: { ...waitEnv, POLYGLOSS_WAIT_TIMEOUT_S: "31" },
      });
      expect(env.exitCode).toBe(0);
      expect(env.ms).toBeGreaterThanOrEqual(800);
      expect(env.ms).toBeLessThan(10_000);
    },
    SLOW,
  );

  test(
    "does not re-fire for an already woken seq",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      const { seq } = submit(reviewId, "Wake once.");
      const first = await runWait(["--session", session]);
      expect(first.exitCode).toBe(2);
      expect(
        query<{ last_woken_seq: number }>(
          "SELECT last_woken_seq FROM sessions WHERE id = ?",
          session,
        )?.last_woken_seq,
      ).toBe(seq);

      const again = await runWait(["--session", session, "--timeout", "31"]);
      expect(again.exitCode).toBe(0);
      expect(again.stderr).toBe("");

      // A later submission wakes it again.
      submit(reviewId, "Second round.", "comment");
      const later = await runWait(["--session", session]);
      expect(later.exitCode).toBe(2);
      expect(later.stderr).toContain("Second round.");
      expect(later.stderr).not.toContain("Wake once.");
    },
    SLOW,
  );

  test(
    "follows canonical session after id drift",
    async () => {
      // The MCP server recorded session A with this test process as its owner
      // (the Claude Code process); after `/clear` the hook reports a new id B.
      const sessionA = newSession();
      const sessionB = newSession();
      const { reviewId } = assignedReview(sessionA, [
        "--owner-pid",
        String(process.pid),
      ]);
      submit(reviewId, "Seen through the canonical id.");

      // No owner override: the waiter records its real parent, this process.
      const r = await runWait(["--session", sessionB], { env: testEnv });
      expect(r.exitCode).toBe(2);
      expect(r.stderr).toContain("Seen through the canonical id.");
      const b = query<{ canonical_id: string; owner_pid: number }>(
        "SELECT canonical_id, owner_pid FROM sessions WHERE id = ?",
        sessionB,
      );
      expect(b).toEqual({ canonical_id: sessionA, owner_pid: process.pid });
      // The wake is recorded on the canonical session.
      expect(
        query<{ client_name: string; last_woken_seq: number }>(
          "SELECT client_name, last_woken_seq FROM sessions WHERE id = ?",
          sessionA,
        ),
      ).toMatchObject({ client_name: "claude-code" });
      expect(
        query<{ last_woken_seq: number }>(
          "SELECT last_woken_seq FROM sessions WHERE id = ?",
          sessionA,
        )!.last_woken_seq,
      ).toBeGreaterThan(0);
    },
    SLOW,
  );

  test(
    "plain agent reply does not wake",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      const waiter = startWait(["--session", session, "--timeout", "34"]);
      await registered(waiter.proc.pid);
      insertEvent("comment.created", reviewId, null);
      insertEvent("thread.resolved", reviewId, null);
      const r = await waiter.done;
      expect(r.exitCode).toBe(0);
      expect(r.stderr).toBe("");
    },
    SLOW,
  );

  test(
    "a submission already answered by request_rereview does not wake",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      submit(reviewId, "Old feedback.");
      insertEvent("review.rereview_requested", reviewId, { summary: "Fixed." });
      const r = await runWait(["--session", session, "--timeout", "31"]);
      expect(r.exitCode).toBe(0);
      expect(r.stderr).toBe("");
    },
    SLOW,
  );

  test(
    "an approval wakes the session too",
    async () => {
      const session = newSession();
      const { reviewId } = assignedReview(session);
      submit(reviewId, "Ship it.", "approve");
      const r = await runWait(["--session", session]);
      expect(r.exitCode).toBe(2);
      expect(r.stderr).toContain("approve");
      expect(r.stderr).toContain("Ship it.");
    },
    SLOW,
  );

  test(
    "exit 1 with message on unreadable db",
    async () => {
      const broken = join(sandbox.home, "broken-data");
      mkdirSync(broken, { recursive: true, mode: 0o700 });
      writeFileSync(
        join(broken, "polygloss.db"),
        "this is not a sqlite database, just text\n".repeat(200),
      );
      const r = await runWait(["--session", newSession()], {
        env: { ...waitEnv, POLYGLOSS_DATA_DIR: broken },
      });
      expect(r.exitCode).toBe(1);
      expect(r.stderr).toContain("polygloss: ");
      expect(r.stderr.toLowerCase()).toContain("database");
    },
    SLOW,
  );

  test(
    "exit 1 without any session id",
    async () => {
      const r = await runWait([]);
      expect(r.exitCode).toBe(1);
      expect(r.stderr).toContain("--session");
    },
    SLOW,
  );
});
