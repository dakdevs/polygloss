// `wait_for_review` (T4.7, design §15.2): the long-poll a client without the
// Polygloss plugin uses, driven through a real `polygloss-cli mcp`. The human
// (submit, comment, archive) is played by the hidden `polygloss-cli debug`
// commands in separate processes, so every wake-up crosses processes the way
// the app's writes do.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { connectMcp, initializeRequest, rawMcpSession } from "../support/mcp";
import { makeSandbox } from "../support/sandbox";

setDefaultTimeout(60_000);

const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

type Json = Record<string, unknown>;
type Env = Record<string, string>;

function world(extra: Env = {}): { env: Env; home: string } {
  const sandbox = makeSandbox();
  sandboxes.push(sandbox);
  const fakeApp = join(sandbox.home, "fake-app.sh");
  writeFileSync(fakeApp, "#!/bin/sh\nexit 0\n");
  chmodSync(fakeApp, 0o755);
  return {
    env: {
      ...sandbox.env,
      POLYGLOSS_TEST: "1",
      POLYGLOSS_APP_BIN: fakeApp,
      CLAUDE_CODE_SESSION_ID: "sess-me",
      ...extra,
    },
    home: sandbox.home,
  };
}

function git(env: Env, repo: string, args: string[]): void {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
}

function debug(env: Env, args: string[]): Json {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
  if (r.exitCode !== 0)
    throw new Error(`debug ${args.join(" ")}: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString()) as Json;
}

/** The same `debug` command in the background, after `delayMs`. */
async function debugLater(
  env: Env,
  args: string[],
  delayMs: number,
): Promise<Json> {
  await Bun.sleep(delayMs);
  const proc = Bun.spawn([cliBin(), "debug", ...args], {
    env,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [out, err, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (code !== 0) throw new Error(`debug ${args.join(" ")}: ${err}`);
  return JSON.parse(out) as Json;
}

/** A live review (since HEAD) of a repo whose `a.txt` line 5 changed. */
function liveReview(env: Env, home: string): string {
  const path = join(home, "repo");
  mkdirSync(path, { recursive: true });
  git(env, path, ["init", "-q", "-b", "main"]);
  const lines = Array.from({ length: 10 }, (_, i) => `l${i + 1}`);
  writeFileSync(join(path, "a.txt"), `${lines.join("\n")}\n`);
  git(env, path, ["add", "."]);
  git(env, path, ["commit", "-q", "-m", "init"]);
  lines[4] = "L5";
  writeFileSync(join(path, "a.txt"), `${lines.join("\n")}\n`);
  const seeded = debug(env, ["seed", "--repo", path, "--since", "HEAD"]);
  return seeded.review_id as string;
}

type Mcp = Awaited<ReturnType<typeof connectMcp>>;

async function withMcp<T>(env: Env, f: (mcp: Mcp) => Promise<T>): Promise<T> {
  const mcp = await connectMcp({ env, clientName: "claude-code" });
  try {
    return await f(mcp);
  } finally {
    await mcp.close();
  }
}

async function call(
  mcp: Mcp,
  name: string,
  args: Json,
  options: {
    onprogress?: (p: { progress: number; total?: number }) => void;
    signal?: AbortSignal;
  } = {},
): Promise<Json> {
  const res = await mcp.client.callTool(
    { name, arguments: args },
    { timeout: 120_000, ...options },
  );
  if (res.isError)
    throw new Error(`${name}: ${JSON.stringify(res.structuredContent)}`);
  return res.structuredContent as Json;
}

async function latestSeq(mcp: Mcp, reviewId: string): Promise<number> {
  const listed = await call(mcp, "list_threads", { review_id: reviewId });
  return listed.latest_seq as number;
}

describe("wait_for_review", () => {
  test("returns immediately when a submission exists after since", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    await withMcp(env, async (mcp) => {
      const since = await latestSeq(mcp, reviewId);
      const submitted = debug(env, [
        "human-submit",
        "--review",
        reviewId,
        "--verdict",
        "approve",
        "--summary",
        "Ship it.",
      ]);
      const started = performance.now();
      const r = await call(mcp, "wait_for_review", {
        review_id: reviewId,
        since,
      });
      expect(performance.now() - started).toBeLessThan(3_000);
      expect(r.outcome).toBe("submitted");
      const submission = r.submission as Json;
      expect(submission.submission_id).toBe(submitted.submission_id);
      expect(submission.verdict).toBe("approve");
      expect(submission.summary_md).toBe("Ship it.");
      expect(submission.iteration).toBe(1);
      expect(submission.at as string).toMatch(RFC3339);
      expect(r.threads).toEqual([]);
      expect(r.next_since as number).toBeGreaterThanOrEqual(
        submitted.seq as number,
      );
    });
  });

  test("default since is now, so an earlier submission does not return", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    debug(env, ["human-submit", "--review", reviewId]);
    await withMcp(env, async (mcp) => {
      const seq = await latestSeq(mcp, reviewId);
      const r = await call(mcp, "wait_for_review", {
        review_id: reviewId,
        timeout_s: 1,
      });
      expect(r).toEqual({ outcome: "timeout", next_since: seq });
    });
  });

  test("blocks until a submit from another process", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    await withMcp(env, async (mcp) => {
      const since = await latestSeq(mcp, reviewId);
      const started = performance.now();
      const [r, submitted] = await Promise.all([
        call(mcp, "wait_for_review", { review_id: reviewId, since }),
        debugLater(
          env,
          ["human-submit", "--review", reviewId, "--summary", "Fix the name."],
          800,
        ),
      ]);
      expect(performance.now() - started).toBeGreaterThanOrEqual(700);
      expect(r.outcome).toBe("submitted");
      const submission = r.submission as Json;
      expect(submission.submission_id).toBe(submitted.submission_id);
      expect(submission.verdict).toBe("request_changes");
      expect(submission.summary_md).toBe("Fix the name.");
    });
  });

  test("timeout returns outcome timeout with next_since", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    await withMcp(env, async (mcp) => {
      const seq = await latestSeq(mcp, reviewId);
      const started = performance.now();
      const r = await call(mcp, "wait_for_review", {
        review_id: reviewId,
        timeout_s: 1,
      });
      const took = performance.now() - started;
      expect(took).toBeGreaterThanOrEqual(1_000);
      expect(took).toBeLessThan(5_000);
      expect(r).toEqual({ outcome: "timeout", next_since: seq });
      // `next_since` carries over: a submission after it is found at once.
      const submitted = debug(env, ["human-submit", "--review", reviewId]);
      const again = await call(mcp, "wait_for_review", {
        review_id: reviewId,
        since: r.next_since,
        timeout_s: 1,
      });
      expect((again.submission as Json).submission_id).toBe(
        submitted.submission_id,
      );
    });
  });

  test("archived returns outcome archived", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    await withMcp(env, async (mcp) => {
      const since = await latestSeq(mcp, reviewId);
      const [r] = await Promise.all([
        call(mcp, "wait_for_review", { review_id: reviewId, since }),
        debugLater(env, ["human-archive", "--review", reviewId], 500),
      ]);
      expect(r.outcome).toBe("archived");
      expect(r.next_since as number).toBeGreaterThan(since);
      // Waiting on an archived review returns at once.
      const started = performance.now();
      const again = await call(mcp, "wait_for_review", {
        review_id: reviewId,
      });
      expect(performance.now() - started).toBeLessThan(3_000);
      expect(again.outcome).toBe("archived");
    });
  });

  test("progress notifications sent with progressToken", async () => {
    const { env, home } = world({ POLYGLOSS_WAIT_PROGRESS_MS: "200" });
    const reviewId = liveReview(env, home);
    await withMcp(env, async (mcp) => {
      const seen: { progress: number; total?: number }[] = [];
      const r = await call(
        mcp,
        "wait_for_review",
        { review_id: reviewId, timeout_s: 2 },
        { onprogress: (p) => seen.push(p) },
      );
      expect(r.outcome).toBe("timeout");
      expect(seen.length).toBeGreaterThanOrEqual(3);
      expect(seen.length).toBeLessThanOrEqual(11);
      for (let i = 1; i < seen.length; i++)
        expect(seen[i]!.progress).toBeGreaterThan(seen[i - 1]!.progress);
      for (const p of seen) expect(p.total).toBe(2);
    });
  });

  test("no progress notifications without a progressToken", async () => {
    const { env, home } = world({ POLYGLOSS_WAIT_PROGRESS_MS: "100" });
    const reviewId = liveReview(env, home);
    const init = initializeRequest(1, "claude-code");
    const lines = [
      init,
      JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
      JSON.stringify({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: {
          name: "wait_for_review",
          arguments: { review_id: reviewId, timeout_s: 1 },
        },
      }),
    ];
    const { stdout } = await rawMcpSession({ env, lines, waitFor: 2 });
    const messages = stdout
      .split("\n")
      .filter((l) => l.trim() !== "")
      .map((l) => JSON.parse(l) as Json);
    expect(
      messages.filter((m) => m.method === "notifications/progress"),
    ).toEqual([]);
    const result = messages.find((m) => m.id === 2)!.result as Json;
    expect((result.structuredContent as Json).outcome).toBe("timeout");
  });

  test("cancellation ends the wait", async () => {
    const { env, home } = world({ RUST_LOG: "polygloss_mcp=debug" });
    const reviewId = liveReview(env, home);
    await withMcp(env, async (mcp) => {
      const abort = new AbortController();
      const pending = call(
        mcp,
        "wait_for_review",
        { review_id: reviewId, timeout_s: 1500 },
        { signal: abort.signal },
      );
      await Bun.sleep(700);
      abort.abort("the user pressed Esc");
      await expect(pending).rejects.toThrow();
      const deadline = Date.now() + 5_000;
      while (
        !mcp.stderr().includes("wait_for_review cancelled") &&
        Date.now() < deadline
      )
        await Bun.sleep(50);
      expect(mcp.stderr()).toContain("wait_for_review cancelled");
      // The server keeps serving.
      expect(await latestSeq(mcp, reviewId)).toBeGreaterThan(0);
    });
  });

  test("closing stdin ends a pending wait", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    const proc = Bun.spawn([cliBin(), "mcp"], {
      env,
      stdin: "pipe",
      stdout: "pipe",
      stderr: "ignore",
    });
    for (const line of [
      initializeRequest(1, "claude-code"),
      JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
      JSON.stringify({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: {
          name: "wait_for_review",
          arguments: { review_id: reviewId, timeout_s: 1500 },
        },
      }),
    ])
      proc.stdin.write(`${line}\n`);
    await proc.stdin.flush();
    await Bun.sleep(1_000);
    const started = performance.now();
    proc.stdin.end();
    const exited = await Promise.race([
      proc.exited.then(() => true),
      Bun.sleep(15_000).then(() => false),
    ]);
    if (!exited) proc.kill();
    expect(exited).toBe(true);
    expect(performance.now() - started).toBeLessThan(15_000);
  });

  test("threads lists new or updated threads since", async () => {
    const { env, home } = world();
    const reviewId = liveReview(env, home);
    const note = (body: string, line: string) =>
      debug(env, [
        "agent-comment",
        "--review",
        reviewId,
        "--body",
        body,
        "--path",
        "a.txt",
        "--line",
        line,
      ]).thread_id as string;
    const untouched = note("Untouched note.", "2");
    const answered = note("Why this name?", "3");
    await withMcp(env, async (mcp) => {
      const since = await latestSeq(mcp, reviewId);
      debug(env, [
        "human-comment",
        "--review",
        reviewId,
        "--reply-to",
        answered,
        "--body",
        "Because.",
      ]);
      const fresh = debug(env, [
        "human-comment",
        "--review",
        reviewId,
        "--path",
        "a.txt",
        "--line",
        "5",
        "--body",
        "Rename this.",
      ]).thread_id as string;
      debug(env, ["human-submit", "--review", reviewId]);
      const r = await call(mcp, "wait_for_review", {
        review_id: reviewId,
        since,
      });
      expect(r.outcome).toBe("submitted");
      const threads = r.threads as Json[];
      expect(threads.map((t) => t.thread_id)).toEqual([answered, fresh]);
      expect(threads.map((t) => t.thread_id)).not.toContain(untouched);
      const listed = await call(mcp, "list_threads", {
        review_id: reviewId,
        since,
        status: "all",
      });
      expect(threads).toEqual(listed.threads as Json[]);
    });
  });

  test("an unknown review is not_found", async () => {
    const { env } = world();
    await withMcp(env, async (mcp) => {
      const res = await mcp.client.callTool({
        name: "wait_for_review",
        arguments: { review_id: "no-such-review", timeout_s: 1 },
      });
      expect(res.isError).toBe(true);
      expect((res.structuredContent as Json).code).toBe("not_found");
    });
  });
});
