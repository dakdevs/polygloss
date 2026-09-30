// MCP read tools and resources (T4.5, design §15.2, §15.3): `list_reviews`,
// `list_threads`, `get_thread` and the `polygloss://` resources, driven through
// a real `polygloss-cli mcp`. The human side (and agent threads, until the
// write tools land) is played by the hidden `polygloss-cli debug` commands.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";
import { makeSandbox } from "../support/sandbox";

// Every test spawns the CLI a few (up to ~120) times to set up its review.
setDefaultTimeout(60_000);

const sandboxes: { cleanup: () => void }[] = [];
afterAll(() => {
  for (const s of sandboxes) s.cleanup();
});

const SESSION = "sess-me";
const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

type Json = Record<string, unknown>;
type Env = Record<string, string>;

/** A fresh sandbox (own store, so sessions never link across tests). */
function world(): { env: Env; home: string } {
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
      CLAUDE_CODE_SESSION_ID: SESSION,
    },
    home: sandbox.home,
  };
}

function git(env: Env, repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
  return r.stdout.toString().trim();
}

const TEN = Array.from({ length: 10 }, (_, i) => `l${i + 1}`);

/** A repo whose `a.txt` has ten lines committed; `edit` writes the worktree. */
function repo(
  env: Env,
  home: string,
  name: string,
): { path: string; write: (file: string, lines: string[]) => void } {
  const path = join(home, name);
  mkdirSync(path, { recursive: true });
  git(env, path, ["init", "-q", "-b", "main"]);
  const write = (file: string, lines: string[]) =>
    writeFileSync(join(path, file), `${lines.join("\n")}\n`);
  write("a.txt", TEN);
  write("b.txt", ["b1", "b2"]);
  git(env, path, ["add", "."]);
  git(env, path, ["commit", "-q", "-m", "init"]);
  return { path, write };
}

function debug(env: Env, args: string[]): Json {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
  if (r.exitCode !== 0)
    throw new Error(`debug ${args.join(" ")}: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString()) as Json;
}

/** A live review of `repo` (since HEAD) with `a.txt` line 5 changed to L5. */
function liveReview(
  env: Env,
  home: string,
  name = "repo",
): { repo: ReturnType<typeof repo>; reviewId: string; diffId: string } {
  const r = repo(env, home, name);
  r.write(
    "a.txt",
    TEN.map((l) => (l === "l5" ? "L5" : l)),
  );
  const seeded = debug(env, ["seed", "--repo", r.path, "--since", "HEAD"]);
  return {
    repo: r,
    reviewId: seeded.review_id as string,
    diffId: seeded.diff_id as string,
  };
}

async function withMcp<T>(
  env: Env,
  f: (call: (name: string, args?: Json) => Promise<Json>) => Promise<T>,
): Promise<T> {
  const mcp = await connectMcp({ env, clientName: "claude-code" });
  try {
    return await f(async (name, args = {}) => {
      const res = await mcp.client.callTool({ name, arguments: args });
      const content = res.structuredContent as Json;
      if (res.isError) return { error: content };
      return content;
    });
  } finally {
    await mcp.close();
  }
}

async function withClient<T>(
  env: Env,
  f: (client: Awaited<ReturnType<typeof connectMcp>>["client"]) => Promise<T>,
): Promise<T> {
  const mcp = await connectMcp({ env, clientName: "claude-code" });
  try {
    return await f(mcp.client);
  } finally {
    await mcp.close();
  }
}

function ok(v: Json): Json {
  if ("error" in v) throw new Error(JSON.stringify(v.error));
  return v;
}

describe("list_reviews", () => {
  test("list_reviews shows viewed counts open questions and last submission", async () => {
    const { env, home } = world();
    const { reviewId, diffId, repo: r } = liveReview(env, home);
    r.write("b.txt", ["b1", "B2"]);
    // Re-seed so the latest iteration has both files.
    const seeded = debug(env, ["seed", "--repo", r.path, "--since", "HEAD"]);
    expect(seeded.review_id).toBe(reviewId);
    debug(env, ["human-viewed", "--review", reviewId, "--path", "a.txt"]);
    debug(env, [
      "agent-comment",
      "--review",
      reviewId,
      "--kind",
      "question",
      "--body",
      "Keep the old name?",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, [
      "agent-comment",
      "--review",
      reviewId,
      "--kind",
      "note",
      "--body",
      "Renamed for clarity.",
    ]);
    debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Please rename.",
      "--path",
      "b.txt",
      "--line",
      "2",
    ]);
    debug(env, [
      "human-submit",
      "--review",
      reviewId,
      "--verdict",
      "request-changes",
      "--summary",
      "Fix the names.",
    ]);

    await withMcp(env, async (call) => {
      const res = ok(await call("list_reviews"));
      const reviews = res.reviews as Json[];
      expect(reviews).toHaveLength(1);
      const review = reviews[0] as Json;
      expect(review.review_id).toBe(reviewId);
      expect(review.kind).toBe("live");
      expect(review.status).toBe("changes_requested");
      expect(review.repo).toBe(r.path);
      expect(typeof review.key).toBe("string");
      expect(review.iterations).toBe(2);
      expect(review.latest_diff_id).toBe(seeded.diff_id);
      expect(review.latest_diff_id).not.toBe(diffId);
      expect(review.viewed).toEqual({ done: 1, total: 2 });
      expect(review.open_threads).toBe(3);
      expect(review.open_questions).toBe(1);
      const last = review.last_submission as Json;
      expect(last.verdict).toBe("request_changes");
      expect(last.summary_md).toBe("Fix the names.");
      expect(last.at as string).toMatch(RFC3339);
      expect(review.updated_at as string).toMatch(RFC3339);
      expect(res.next_cursor).toBeUndefined();
    });
  });

  test("list_reviews assigned me filter", async () => {
    const { env, home } = world();
    const mine = liveReview(env, home, "mine");
    const theirs = liveReview(env, home, "theirs");
    const unassigned = liveReview(env, home, "unassigned");
    debug(env, ["assign", "--review", mine.reviewId, "--session", SESSION]);
    debug(env, [
      "assign",
      "--review",
      theirs.reviewId,
      "--session",
      "sess-other",
    ]);

    await withMcp(env, async (call) => {
      const me = ok(await call("list_reviews", { assigned: "me" }));
      const ids = (me.reviews as Json[]).map((r) => r.review_id);
      expect(ids).toEqual([mine.reviewId]);
      expect((me.reviews as Json[])[0]?.assigned_session).toBe(SESSION);

      const any = ok(await call("list_reviews", { assigned: "any" }));
      expect((any.reviews as Json[]).map((r) => r.review_id).sort()).toEqual(
        [mine.reviewId, theirs.reviewId, unassigned.reviewId].sort(),
      );
      const byRepo = ok(await call("list_reviews", { repo: theirs.repo.path }));
      expect((byRepo.reviews as Json[]).map((r) => r.review_id)).toEqual([
        theirs.reviewId,
      ]);
      const paged = ok(await call("list_reviews", { limit: 2 }));
      expect(paged.reviews as Json[]).toHaveLength(2);
      const rest = ok(
        await call("list_reviews", { limit: 2, cursor: paged.next_cursor }),
      );
      expect(rest.reviews as Json[]).toHaveLength(1);
      expect(rest.next_cursor).toBeUndefined();
      const bad = await call("list_reviews", { status: "nope" });
      expect((bad.error as Json).code).toBe("conflict");
    });
  });
});

describe("list_threads", () => {
  test("list_threads hides human drafts until submitted", async () => {
    const { env, home } = world();
    const { reviewId } = liveReview(env, home);
    const draft = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "draft body",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);

    await withMcp(env, async (call) => {
      const before = ok(await call("list_threads", { review_id: reviewId }));
      expect(before.threads).toEqual([]);
      expect(typeof before.latest_seq).toBe("number");
      const hidden = await call("get_thread", { thread_id: draft.thread_id });
      expect((hidden.error as Json).code).toBe("not_found");
    });

    debug(env, ["human-submit", "--review", reviewId]);

    await withMcp(env, async (call) => {
      const after = ok(await call("list_threads", { review_id: reviewId }));
      const threads = after.threads as Json[];
      expect(threads).toHaveLength(1);
      const t = threads[0] as Json;
      expect(t.thread_id).toBe(draft.thread_id);
      expect(t.review_id).toBe(reviewId);
      expect(t.kind).toBe("comment");
      expect(t.subject).toBe("line");
      expect(t.path).toBe("a.txt");
      expect(t.side).toBe("new");
      expect(t.start_line).toBe(5);
      expect(t.line).toBe(5);
      expect(t.position).toEqual({ state: "exact", start_line: 5, line: 5 });
      expect(t.status).toBe("open");
      expect(t.created_by).toEqual({ kind: "human", name: "you" });
      expect(t.comment_count).toBe(1);
      expect(t.has_suggestion).toBe(false);
      const last = t.last_comment as Json;
      expect(last.author_kind).toBe("human");
      expect(last.author_name).toBe("you");
      expect(last.excerpt).toBe("draft body");
      expect(last.at as string).toMatch(RFC3339);
      expect(t.updated_at as string).toMatch(RFC3339);
    });
  });

  test("list_threads since seq returns only newer", async () => {
    const { env, home } = world();
    const { reviewId, diffId } = liveReview(env, home);
    const human = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "first",
    ]);
    debug(env, ["human-submit", "--review", reviewId]);

    await withMcp(env, async (call) => {
      const first = ok(await call("list_threads", { review_id: reviewId }));
      expect((first.threads as Json[]).map((t) => t.thread_id)).toEqual([
        human.thread_id,
      ]);
      const seq = first.latest_seq as number;

      const note = debug(env, [
        "agent-comment",
        "--review",
        reviewId,
        "--kind",
        "note",
        "--body",
        "later",
        "--path",
        "a.txt",
      ]);
      const newer = ok(
        await call("list_threads", { review_id: reviewId, since: seq }),
      );
      expect((newer.threads as Json[]).map((t) => t.thread_id)).toEqual([
        note.thread_id,
      ]);
      expect(newer.latest_seq as number).toBeGreaterThan(seq);
      const byDiff = ok(
        await call("list_threads", {
          diff_id: diffId.slice(0, 12),
          since: 0,
          author: "agent",
        }),
      );
      expect((byDiff.threads as Json[]).map((t) => t.thread_id)).toEqual([
        note.thread_id,
      ]);
      expect((byDiff.threads as Json[])[0]?.subject).toBe("file");
      const none = ok(
        await call("list_threads", {
          review_id: reviewId,
          since: newer.latest_seq,
        }),
      );
      expect(none.threads).toEqual([]);
    });
  });

  test("list_threads paginates under 60k chars with next_cursor", async () => {
    const { env, home } = world();
    const { reviewId } = liveReview(env, home);
    const count = 120;
    const created: string[] = [];
    for (let i = 0; i < count; i++) {
      const t = debug(env, [
        "human-comment",
        "--review",
        reviewId,
        "--body",
        `${i} ${"x".repeat(2_000)}`,
        "--path",
        "a.txt",
        "--line",
        String((i % 10) + 1),
      ]);
      created.push(t.thread_id as string);
    }
    debug(env, ["human-submit", "--review", reviewId]);

    await withMcp(env, async (call) => {
      const byDefault = ok(await call("list_threads", { review_id: reviewId }));
      expect(byDefault.threads as Json[]).toHaveLength(50);
      expect(typeof byDefault.next_cursor).toBe("string");

      const seen: string[] = [];
      let cursor: unknown;
      let pages = 0;
      for (;;) {
        const page = ok(
          await call("list_threads", {
            review_id: reviewId,
            limit: 200,
            ...(cursor ? { cursor } : {}),
          }),
        );
        pages += 1;
        expect(JSON.stringify(page).length).toBeLessThan(60_000);
        for (const t of page.threads as Json[]) {
          expect((t.last_comment as Json).excerpt as string).toHaveLength(300);
          seen.push(t.thread_id as string);
        }
        cursor = page.next_cursor;
        if (!cursor) break;
      }
      expect(pages).toBeGreaterThan(1);
      expect(seen).toEqual(created);

      const tampered = await call("list_threads", {
        review_id: reviewId,
        cursor: `${byDefault.next_cursor as string}x`,
      });
      expect((tampered.error as Json).code).toBe("conflict");
      const otherQuery = await call("list_threads", {
        review_id: reviewId,
        status: "all",
        cursor: byDefault.next_cursor,
      });
      expect((otherQuery.error as Json).code).toBe("conflict");
    });
  });
});

describe("get_thread", () => {
  test("get_thread returns structured suggestions and diff_hunk", async () => {
    const { env, home } = world();
    const { reviewId, diffId } = liveReview(env, home);
    const t = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Use this:\n\n```suggestion\nL5 fixed\n```\n",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, ["human-submit", "--review", reviewId]);
    const reply = debug(env, [
      "agent-comment",
      "--reply-to",
      t.thread_id as string,
      "--body",
      "Done.",
    ]);

    await withMcp(env, async (call) => {
      const th = ok(await call("get_thread", { thread_id: t.thread_id }));
      expect(th.thread_id).toBe(t.thread_id);
      expect(th.has_suggestion).toBe(true);
      expect(th.comment_count).toBe(2);
      expect(th.origin_diff_id).toBe(diffId);
      expect(th.position).toEqual({ state: "exact", start_line: 5, line: 5 });
      const anchor = th.anchor as Json;
      expect(anchor.path).toBe("a.txt");
      expect(anchor.side).toBe("new");
      expect(anchor.start_line).toBe(5);
      expect(anchor.line).toBe(5);
      expect(anchor.anchor_blob as string).toMatch(/^[0-9a-f]{40}$/);
      expect(anchor.original_snippet).toBe("L5");
      expect(anchor.current_snippet).toBe("L5");
      expect(anchor.diff_hunk).toBe("@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5\n+L5");
      const comments = th.comments as Json[];
      expect(comments).toHaveLength(2);
      expect(comments[0]?.author_kind).toBe("human");
      expect(comments[0]?.suggestions).toEqual([
        { start_line: 5, line: 5, original: "L5", replacement: "L5 fixed" },
      ]);
      expect(comments[0]?.truncated).toBe(false);
      expect(comments[0]?.created_at as string).toMatch(RFC3339);
      expect(comments[1]?.comment_id).toBe(reply.comment_id);
      expect(comments[1]?.author_kind).toBe("agent");
      expect(comments[1]?.author_name).toBe("claude-code");
      expect(comments[1]?.suggestions).toEqual([]);
      const last = th.last_comment as Json;
      expect(last.author_name).toBe("claude-code");
      expect(last.excerpt).toBe("Done.");
    });
  });

  test("get_thread position moved after unrelated edit", async () => {
    const { env, home } = world();
    const { reviewId, repo: r } = liveReview(env, home);
    const t = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Look here",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, ["human-submit", "--review", reviewId]);
    r.write("a.txt", [
      "new1",
      "new2",
      ...TEN.map((l) => (l === "l5" ? "L5" : l)),
    ]);
    debug(env, ["seed", "--repo", r.path, "--since", "HEAD"]);

    await withMcp(env, async (call) => {
      const th = ok(await call("get_thread", { thread_id: t.thread_id }));
      expect(th.position).toEqual({ state: "moved", start_line: 7, line: 7 });
      expect(th.line).toBe(5);
      const anchor = th.anchor as Json;
      expect(anchor.line).toBe(5);
      expect(anchor.original_snippet).toBe("L5");
      expect(anchor.current_snippet).toBe("L5");
      const listed = ok(await call("list_threads", { review_id: reviewId }));
      expect((listed.threads as Json[])[0]?.position).toEqual({
        state: "moved",
        start_line: 7,
        line: 7,
      });
    });
  });

  test("get_thread outdated keeps original snippet", async () => {
    const { env, home } = world();
    const { reviewId, repo: r } = liveReview(env, home);
    const t = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Rename this",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, ["human-submit", "--review", reviewId]);
    r.write(
      "a.txt",
      TEN.map((l) => (l === "l5" ? "L5 renamed" : l)),
    );
    debug(env, ["seed", "--repo", r.path, "--since", "HEAD"]);

    await withMcp(env, async (call) => {
      const th = ok(await call("get_thread", { thread_id: t.thread_id }));
      expect((th.position as Json).state).toBe("outdated");
      const anchor = th.anchor as Json;
      expect(anchor.original_snippet).toBe("L5");
      expect(anchor.current_snippet as string).toContain("L5 renamed");
      expect(anchor.diff_hunk).toBe("@@ -2,7 +2,7 @@\n l2\n l3\n l4\n-l5\n+L5");
    });
  });
});

describe("resources", () => {
  test("resources list assigned plus recent", async () => {
    const { env, home } = world();
    const r = repo(env, home, "many");
    const reviews: string[] = [];
    for (let i = 0; i < 22; i++) {
      r.write("a.txt", [...TEN, `extra ${i}`]);
      git(env, r.path, ["commit", "-q", "-am", `c${i}`]);
      const seeded = debug(env, ["seed", "--repo", r.path, "--commit", "HEAD"]);
      reviews.push(seeded.review_id as string);
    }
    const [oldest, secondOldest] = reviews;
    debug(env, ["assign", "--review", oldest as string, "--session", SESSION]);

    await withClient(env, async (client) => {
      expect(client.getServerCapabilities()?.resources).toBeDefined();
      const { resources } = await client.listResources();
      const uris = resources.map((res) => res.uri);
      expect(uris).toHaveLength(21);
      expect(uris[0]).toBe(`polygloss://review/${oldest}`);
      expect(uris).not.toContain(`polygloss://review/${secondOldest}`);
      for (const id of reviews.slice(2))
        expect(uris).toContain(`polygloss://review/${id}`);
      for (const res of resources) expect(res.mimeType).toBe("text/markdown");

      const { resourceTemplates } = await client.listResourceTemplates();
      expect(resourceTemplates.map((t) => t.uriTemplate).sort()).toEqual([
        "polygloss://diff/{diff_id}",
        "polygloss://review/{review_id}",
        "polygloss://review/{review_id}/threads",
        "polygloss://thread/{thread_id}",
      ]);
    });
  });

  test("resource thread renders markdown", async () => {
    const { env, home } = world();
    const { reviewId, diffId } = liveReview(env, home);
    const t = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Use this:\n\n```suggestion\nL5 fixed\n```\n",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, [
      "human-submit",
      "--review",
      reviewId,
      "--verdict",
      "comment",
      "--summary",
      "One nit.",
    ]);
    debug(env, [
      "agent-comment",
      "--reply-to",
      t.thread_id as string,
      "--body",
      "Applied the suggestion.",
    ]);

    await withClient(env, async (client) => {
      const read = async (uri: string) => {
        const res = await client.readResource({ uri });
        expect(res.contents).toHaveLength(1);
        const c = res.contents[0] as {
          uri: string;
          mimeType?: string;
          text?: string;
        };
        expect(c.uri).toBe(uri);
        expect(c.mimeType).toBe("text/markdown");
        return c.text ?? "";
      };
      const thread = await read(`polygloss://thread/${t.thread_id}`);
      expect(thread).toStartWith("# ");
      expect(thread).toContain("a.txt:5");
      expect(thread).toContain("```diff\n@@ -2,7 +2,7 @@");
      expect(thread).toContain("Use this:");
      expect(thread).toContain("```suggestion\nL5 fixed\n```");
      expect(thread).toContain("claude-code");
      expect(thread).toContain("Applied the suggestion.");
      expect(thread).toContain("open");

      const review = await read(`polygloss://review/${reviewId}`);
      expect(review).toStartWith("# ");
      expect(review).toContain("commented");
      expect(review).toContain("One nit.");
      expect(review).toContain(reviewId);

      const digest = await read(`polygloss://review/${reviewId}/threads`);
      expect(digest).toContain("a.txt:5");
      expect(digest).toContain("suggestion");
      expect(digest).toContain("Applied the suggestion.");

      const diff = await read(`polygloss://diff/${diffId}`);
      expect(diff).toContain("a.txt");
      expect(diff).toContain("modified");
      expect(diff).toContain("+1");
      expect(diff).toContain("-1");

      await expect(
        client.readResource({ uri: "polygloss://thread/nope" }),
      ).rejects.toThrow();
    });
  });
});
