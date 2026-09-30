// Opt-in `claude/channel` push (T4.13, design §16.4, OQ-33): with
// `polygloss mcp --channel` (or POLYGLOSS_MCP_CHANNEL=1) the server declares
// `experimental["claude/channel"]` and sends one
// `notifications/claude/channel` per submission of a review assigned to its
// session, with the text `polygloss wait` prints. Without the flag nothing
// changes. The human is played by the hidden `polygloss-cli debug` commands in
// separate processes, so every push crosses processes like the app's writes.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";
import { makeSandbox } from "../support/sandbox";

setDefaultTimeout(60_000);

const CHANNEL_METHOD = "notifications/claude/channel";
const ME = "sess-channel-me";
const OTHER = "sess-channel-other";
// Longer than several feed polls (250 ms), so a push that was going to
// happen has happened.
const QUIET_MS = 1_500;

const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

type Json = Record<string, unknown>;
type Env = Record<string, string>;
type Pushed = { content: string; meta: Record<string, string> };

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
      CLAUDE_CODE_SESSION_ID: ME,
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

let repoCount = 0;

/** A live review of a fresh repo with an edited `a.txt`, assigned to `session`. */
function assignedReview(env: Env, home: string, session: string): string {
  repoCount += 1;
  const path = join(home, `repo-${repoCount}`);
  mkdirSync(path, { recursive: true });
  git(env, path, ["init", "-q", "-b", "main"]);
  writeFileSync(join(path, "a.txt"), "one\ntwo\nthree\n");
  git(env, path, ["add", "."]);
  git(env, path, ["commit", "-q", "-m", "init"]);
  writeFileSync(join(path, "a.txt"), "one\nTWO\nthree\nfour\n");
  const seeded = debug(env, ["seed", "--repo", path, "--since", "HEAD"]);
  const reviewId = seeded.review_id as string;
  debug(env, [
    "assign",
    "--review",
    reviewId,
    "--session",
    session,
    "--client",
    "claude-code",
  ]);
  return reviewId;
}

function submit(
  env: Env,
  reviewId: string,
  summary: string,
  verdict = "request-changes",
): Json {
  return debug(env, [
    "human-submit",
    "--review",
    reviewId,
    "--verdict",
    verdict,
    "--summary",
    summary,
  ]);
}

type Mcp = Awaited<ReturnType<typeof connectMcp>> & {
  pushed: Pushed[];
  /** Resolves once `n` pushes arrived (rejects after `ms`). */
  waitFor: (n: number, ms?: number) => Promise<void>;
};

async function withMcp<T>(
  env: Env,
  args: string[],
  f: (mcp: Mcp) => Promise<T>,
): Promise<T> {
  const base = await connectMcp({ env, args, clientName: "claude-code" });
  const pushed: Pushed[] = [];
  const others: string[] = [];
  base.client.fallbackNotificationHandler = async (n) => {
    if (n.method === CHANNEL_METHOD) pushed.push(n.params as Pushed);
    else others.push(n.method);
  };
  const mcp: Mcp = {
    ...base,
    pushed,
    waitFor: async (n, ms = 10_000) => {
      const deadline = Date.now() + ms;
      while (pushed.length < n) {
        if (Date.now() > deadline)
          throw new Error(
            `expected ${n} channel notifications, got ${pushed.length}; stderr: ${base.stderr()}`,
          );
        await Bun.sleep(25);
      }
    },
  };
  try {
    return await f(mcp);
  } finally {
    await mcp.close();
  }
}

/** Gives the server a moment to open its feed after `initialized`. */
async function settle(): Promise<void> {
  await Bun.sleep(300);
}

describe("claude/channel push", () => {
  test("channel capability absent by default", async () => {
    const { env, home } = world();
    await withMcp(env, [], async (mcp) => {
      const caps = mcp.client.getServerCapabilities() ?? {};
      expect(caps.experimental?.["claude/channel"]).toBeUndefined();
      expect(caps.tools).toBeDefined();

      // Nothing is pushed either.
      const reviewId = assignedReview(env, home, ME);
      await settle();
      submit(env, reviewId, "Fix it.");
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed).toEqual([]);
    });
  });

  test("--channel and POLYGLOSS_MCP_CHANNEL=1 declare the capability", async () => {
    const { env } = world();
    await withMcp(env, ["--channel"], async (mcp) => {
      const caps = mcp.client.getServerCapabilities() ?? {};
      expect(caps.experimental?.["claude/channel"]).toEqual({});
      expect(caps.tools).toBeDefined();
    });
    await withMcp({ ...env, POLYGLOSS_MCP_CHANNEL: "1" }, [], async (mcp) => {
      const caps = mcp.client.getServerCapabilities() ?? {};
      expect(caps.experimental?.["claude/channel"]).toEqual({});
    });
    await withMcp({ ...env, POLYGLOSS_MCP_CHANNEL: "0" }, [], async (mcp) => {
      const caps = mcp.client.getServerCapabilities() ?? {};
      expect(caps.experimental?.["claude/channel"]).toBeUndefined();
    });
  });

  test("with --channel a submit sends one claude/channel notification with review_id meta", async () => {
    const { env, home } = world();
    const reviewId = assignedReview(env, home, ME);
    await withMcp(env, ["--channel"], async (mcp) => {
      await settle();
      const submitted = submit(env, reviewId, "Rename the helper.");
      await mcp.waitFor(1);
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed).toHaveLength(1);
      const [push] = mcp.pushed;
      expect(push?.meta).toEqual({
        review_id: reviewId,
        submission_id: submitted.submission_id as string,
        verdict: "request_changes",
      });
      expect(push?.content).toContain(
        "Polygloss: the human submitted their review of",
      );
      expect(push?.content).toContain("Verdict: request changes");
      expect(push?.content).toContain("Summary: Rename the helper.");
      expect(push?.content).toContain(`list_threads(review_id="${reviewId}")`);

      // The same text `polygloss wait` prints, and the push does not use up
      // the Stop hook's wake (it cannot know the client delivered it).
      const waited = Bun.spawnSync(
        [cliBin(), "wait", "--session", ME, "--timeout", "40"],
        { env: { ...env, POLYGLOSS_WAIT_OWNER_PID: "0" }, stdin: "ignore" },
      );
      expect(waited.exitCode).toBe(2);
      expect(waited.stderr.toString().trimEnd()).toBe(push?.content ?? "");
    });
  });

  test("an approval is pushed too and each submission is pushed once", async () => {
    const { env, home } = world();
    const reviewId = assignedReview(env, home, ME);
    await withMcp(env, ["--channel"], async (mcp) => {
      await settle();
      const first = submit(env, reviewId, "First pass.");
      await mcp.waitFor(1);
      const second = submit(env, reviewId, "", "approve");
      await mcp.waitFor(2);
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed.map((p) => p.meta.submission_id)).toEqual([
        first.submission_id as string,
        second.submission_id as string,
      ]);
      expect(mcp.pushed[1]?.meta.verdict).toBe("approve");
      expect(mcp.pushed[1]?.content).toContain("Summary: (none)");
    });
  });

  test("no notification for a review assigned to another session", async () => {
    const { env, home } = world();
    const theirs = assignedReview(env, home, OTHER);
    const mine = assignedReview(env, home, ME);
    await withMcp(env, ["--channel"], async (mcp) => {
      await settle();
      submit(env, theirs, "Not for you.");
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed).toEqual([]);
      // A later submission on this session's review still arrives, alone.
      submit(env, mine, "For you.");
      await mcp.waitFor(1);
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed.map((p) => p.meta.review_id)).toEqual([mine]);
    });
  });

  test("no notification for plain agent replies", async () => {
    const { env, home } = world();
    const reviewId = assignedReview(env, home, ME);
    await withMcp(env, ["--channel"], async (mcp) => {
      await settle();
      const created = await mcp.client.callTool({
        name: "create_comment",
        arguments: {
          review_id: reviewId,
          kind: "note",
          anchor: { path: "a.txt", line: 2 },
          body_md: "Renamed for clarity.",
        },
      });
      expect(created.isError).toBeFalsy();
      const threadId = (created.structuredContent as Json).thread_id as string;
      const replied = await mcp.client.callTool({
        name: "reply",
        arguments: {
          thread_id: threadId,
          body_md: "Also updated the caller.",
          resolve: true,
        },
      });
      expect(replied.isError).toBeFalsy();
      // An agent reply through the debug surface (another process) too.
      debug(env, [
        "agent-comment",
        "--reply-to",
        threadId,
        "--body",
        "One more thing.",
      ]);
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed).toEqual([]);

      // The feed is live: a submission after the replies is the only push.
      const submitted = submit(env, reviewId, "Looks right.", "comment");
      await mcp.waitFor(1);
      await Bun.sleep(QUIET_MS);
      expect(mcp.pushed.map((p) => p.meta.submission_id)).toEqual([
        submitted.submission_id as string,
      ]);
    });
  });
});
