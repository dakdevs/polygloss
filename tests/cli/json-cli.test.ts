// The JSON CLI (T4.9, design §14 "JSON CLI for non-MCP agents"): `reviews`,
// `threads`, `thread`, `reply`, `resolve`, `unresolve`, `edit`, `delete`,
// `comment`, `wait-review`, `rereview` and `focus` call the same
// `polygloss_mcp::api` functions as their MCP twins and print the same JSON.
// The human side is played by the hidden `polygloss-cli debug` commands; the
// app, when one is needed, by a fake bound at the sandbox socket. Nothing here
// launches the installed app: every environment sets POLYGLOSS_TEST=1, and
// POLYGLOSS_APP_BIN only ever names a fake.
import { Database } from "bun:sqlite";
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { connectMcp } from "../support/mcp";
import { makeSandbox } from "../support/sandbox";

// Live opens snapshot the worktree with several git calls in a debug build.
setDefaultTimeout(60_000);

const cleanups: (() => void)[] = [];
afterAll(() => {
  for (const c of cleanups) c();
});

type Json = Record<string, any>;
type Env = Record<string, string>;

/** A sandbox with a data dir short enough for the socket path, in test mode
 * with no app override (an accidental launch fails at once) and no agent or
 * session variables. */
function world(): { env: Env; home: string; dataDir: string } {
  const sandbox = makeSandbox();
  const dataDir = realpathSync(mkdtempSync("/tmp/pgj-"));
  cleanups.push(() => {
    sandbox.cleanup();
    rmSync(dataDir, { recursive: true, force: true });
  });
  return {
    env: { ...sandbox.env, POLYGLOSS_DATA_DIR: dataDir, POLYGLOSS_TEST: "1" },
    home: sandbox.home,
    dataDir,
  };
}

function git(env: Env, repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
  return r.stdout.toString().trim();
}

const TEN = Array.from({ length: 10 }, (_, i) => `l${i + 1}`);

/** A repo with `a.txt` (ten lines) committed and line 5 edited in the
 * worktree. */
function repo(env: Env, home: string, name = "repo"): string {
  const path = join(home, name);
  mkdirSync(path, { recursive: true });
  git(env, path, ["init", "-q", "-b", "main"]);
  writeFileSync(join(path, "a.txt"), `${TEN.join("\n")}\n`);
  git(env, path, ["add", "."]);
  git(env, path, ["commit", "-q", "-m", "init"]);
  writeFileSync(
    join(path, "a.txt"),
    `${TEN.map((l) => (l === "l5" ? "L5" : l)).join("\n")}\n`,
  );
  return realpathSync(path);
}

function debug(env: Env, args: string[]): Json {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
  if (r.exitCode !== 0)
    throw new Error(`debug ${args.join(" ")}: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString()) as Json;
}

/** A pinned live review of a fresh repo (since HEAD), as the human's seed. */
function liveReview(
  env: Env,
  home: string,
): { path: string; reviewId: string; diffId: string } {
  const path = repo(env, home);
  const seeded = debug(env, ["seed", "--repo", path, "--since", "HEAD"]);
  return {
    path,
    reviewId: seeded.review_id as string,
    diffId: seeded.diff_id as string,
  };
}

/** Runs `polygloss-cli <args>` (a JSON command) with `stdin`, and parses its
 * one line of JSON output. */
function pg(
  env: Env,
  args: string[],
  stdin?: string,
): { json: Json; exitCode: number; stderr: string; stdout: string } {
  const r = Bun.spawnSync([cliBin(), ...args], {
    env,
    stdin: stdin === undefined ? "ignore" : new TextEncoder().encode(stdin),
  });
  const stdout = r.stdout.toString();
  let json: Json = {};
  try {
    json = JSON.parse(stdout) as Json;
  } catch {
    // Left empty; the caller checks exitCode and stdout.
  }
  return { json, exitCode: r.exitCode, stderr: r.stderr.toString(), stdout };
}

/** `pg` that must succeed. */
function pgOk(env: Env, args: string[], stdin?: string): Json {
  const r = pg(env, args, stdin);
  if (r.exitCode !== 0)
    throw new Error(
      `polygloss ${args.join(" ")} exited ${r.exitCode}: ${r.stdout}${r.stderr}`,
    );
  expect(r.stdout.endsWith("\n")).toBe(true);
  expect(r.stdout.trim().split("\n")).toHaveLength(1);
  return r.json;
}

type Call = (name: string, args?: Json) => Promise<Json>;

async function withMcp<T>(
  env: Env,
  f: (call: Call) => Promise<T>,
  clientName = "claude-code",
): Promise<T> {
  const mcp = await connectMcp({ env, clientName });
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

function ok(v: Json): Json {
  if ("error" in v) throw new Error(JSON.stringify(v.error));
  return v;
}

function query<T>(dataDir: string, sql: string, ...params: any[]): T[] {
  const db = new Database(join(dataDir, "polygloss.db"), { readwrite: true });
  try {
    return db.query(sql).all(...params) as T[];
  } finally {
    db.close();
  }
}

/** A fake app bound at `<dataDir>/polygloss.sock` that records its launch and
 * every op, answering each with ok. */
function withAppBin(env: Env, dataDir: string): Env {
  const source = `
import { appendFileSync } from "node:fs";
const dir = process.env.POLYGLOSS_DATA_DIR;
appendFileSync(dir + "/launched.log", JSON.stringify({
  activate: process.env.POLYGLOSS_LAUNCH_ACTIVATE ?? null,
}) + "\\n");
let buffered = "";
Bun.listen({
  unix: dir + "/polygloss.sock",
  socket: {
    data(socket, data) {
      buffered += data.toString();
      let nl;
      while ((nl = buffered.indexOf("\\n")) >= 0) {
        const line = buffered.slice(0, nl);
        buffered = buffered.slice(nl + 1);
        const req = JSON.parse(line);
        appendFileSync(dir + "/ops.log", line + "\\n");
        const result = req.op === "hello"
          ? { app: "polygloss", version: "0", pid: process.pid, protocol: 1 }
          : { status: "ok" };
        socket.write(JSON.stringify({ id: req.id, ok: true, result }) + "\\n");
      }
    },
  },
});
setTimeout(() => process.exit(0), 8000);
`;
  const bin = join(dataDir, "fake-polygloss");
  writeFileSync(bin, `#!${process.execPath}\n${source}`);
  chmodSync(bin, 0o755);
  return { ...env, POLYGLOSS_APP_BIN: bin };
}

function lines(file: string): Json[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf8")
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((l) => JSON.parse(l) as Json);
}

const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

describe("read commands", () => {
  test("reviews json equals list_reviews structuredContent", async () => {
    const { env, home } = world();
    const path = repo(env, home);
    const mine = { ...env, CLAUDE_CODE_SESSION_ID: "sess-me" };
    const opened = await withMcp(mine, async (call) =>
      ok(
        await call("open_diff", {
          repo: path,
          source: { kind: "live", since: "HEAD" },
          show: false,
        }),
      ),
    );
    const other = liveReview(env, home.concat("/other"));

    const all = pgOk(env, ["reviews"]);
    const mcpAll = await withMcp(env, async (call) =>
      ok(await call("list_reviews", {})),
    );
    expect(all).toEqual(mcpAll);
    expect((all.reviews as Json[]).map((r) => r.review_id).sort()).toEqual(
      [opened.review_id, other.reviewId].sort(),
    );

    // --repo limits to one repository, like list_reviews(repo).
    const one = pgOk(env, ["--repo", path, "reviews"]);
    expect((one.reviews as Json[]).map((r) => r.review_id)).toEqual([
      opened.review_id,
    ]);

    // --assigned me uses --session.
    const assigned = pgOk(env, [
      "reviews",
      "--assigned",
      "me",
      "--session",
      "sess-me",
    ]);
    const mcpAssigned = await withMcp(mine, async (call) =>
      ok(await call("list_reviews", { assigned: "me" })),
    );
    expect(assigned).toEqual(mcpAssigned);
    expect((assigned.reviews as Json[]).map((r) => r.review_id)).toEqual([
      opened.review_id,
    ]);

    // --assigned me without any session is an error, not an empty list.
    const noSession = pg(env, ["reviews", "--assigned", "me"]);
    expect(noSession.exitCode).toBe(1);
    expect(noSession.json.error.code).toBe("conflict");

    // An unknown status is the api's conflict error, as JSON on stdout.
    const bad = pg(env, ["reviews", "--status", "bogus"]);
    expect(bad.exitCode).toBe(1);
    expect(bad.json.error.code).toBe("conflict");
  });

  test("threads paginates with cursor", async () => {
    const { env, home } = world();
    const { reviewId } = liveReview(env, home);
    const created: string[] = [];
    for (let i = 0; i < 120; i++) {
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

    const first = pgOk(env, ["threads", reviewId]);
    expect(first.threads as Json[]).toHaveLength(50);
    expect(typeof first.next_cursor).toBe("string");
    expect(typeof first.latest_seq).toBe("number");

    const seen: string[] = [];
    let cursor: string | undefined;
    let pages = 0;
    for (;;) {
      const page = pgOk(env, [
        "threads",
        reviewId,
        "--limit",
        "200",
        ...(cursor ? ["--cursor", cursor] : []),
      ]);
      pages += 1;
      expect(JSON.stringify(page).length).toBeLessThan(60_000);
      for (const t of page.threads as Json[]) seen.push(t.thread_id as string);
      cursor = page.next_cursor as string | undefined;
      if (!cursor) break;
    }
    expect(pages).toBeGreaterThan(1);
    expect(seen).toEqual(created);

    // The JSON CLI and MCP continue each other's pages with the same result.
    const second = pgOk(env, [
      "threads",
      reviewId,
      "--cursor",
      first.next_cursor as string,
    ]);
    await withMcp(env, async (call) => {
      expect(
        ok(
          await call("list_threads", {
            review_id: reviewId,
            cursor: first.next_cursor,
          }),
        ),
      ).toEqual(second);
    });

    // Filters pass through: nothing is resolved, nothing by an agent.
    expect(
      pgOk(env, ["threads", reviewId, "--status", "resolved"]).threads,
    ).toEqual([]);
    expect(
      pgOk(env, ["threads", reviewId, "--author", "agent"]).threads,
    ).toEqual([]);
    const tampered = pg(env, [
      "threads",
      reviewId,
      "--cursor",
      `${first.next_cursor as string}x`,
    ]);
    expect(tampered.exitCode).toBe(1);
    expect(tampered.json.error.code).toBe("conflict");
  });

  test("thread equals get_thread", async () => {
    const { env, home } = world();
    const { reviewId } = liveReview(env, home);
    const t = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Use this:\n```suggestion\nL5 fixed\n```",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, ["human-submit", "--review", reviewId]);
    const cli = pgOk(env, ["thread", t.thread_id]);
    const mcp = await withMcp(env, async (call) =>
      ok(await call("get_thread", { thread_id: t.thread_id })),
    );
    expect(cli).toEqual(mcp);
    expect(cli.anchor.diff_hunk).toStartWith("@@");
    expect(cli.comments[0].suggestions).toHaveLength(1);

    const missing = pg(env, ["thread", "no-such-thread"]);
    expect(missing.exitCode).toBe(1);
    expect(missing.json.error.code).toBe("not_found");
  });

  test("wait-review times out", async () => {
    const { env, home } = world();
    const { reviewId } = liveReview(env, home);
    const started = performance.now();
    const r = pgOk(env, ["wait-review", reviewId, "--timeout", "1"]);
    expect(performance.now() - started).toBeGreaterThanOrEqual(900);
    expect(r.outcome).toBe("timeout");
    expect(typeof r.next_since).toBe("number");

    // A submission after --since returns at once, like wait_for_review.
    const submitted = debug(env, [
      "human-submit",
      "--review",
      reviewId,
      "--verdict",
      "approve",
      "--summary",
      "Ship it.",
    ]);
    const done = pgOk(env, [
      "wait-review",
      reviewId,
      "--since",
      String(r.next_since),
      "--timeout",
      "30",
    ]);
    expect(done.outcome).toBe("submitted");
    expect(done.submission.submission_id).toBe(submitted.submission_id);
    expect(done.submission.summary_md).toBe("Ship it.");
  });
});

describe("write commands", () => {
  test("reply reads body from stdin", async () => {
    const { env, home, dataDir } = world();
    const { reviewId } = liveReview(env, home);
    const t = debug(env, [
      "human-comment",
      "--review",
      reviewId,
      "--body",
      "Why?",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, ["human-submit", "--review", reviewId]);

    const r = pgOk(
      env,
      ["reply", t.thread_id, "--body-file", "-", "--session", "sess-cli"],
      "Because **reasons**.\n",
    );
    expect(r).toEqual({
      comment_id: expect.any(String),
      thread_id: t.thread_id,
      status: "open",
    });
    const thread = pgOk(env, ["thread", t.thread_id]);
    expect(thread.comments[1]).toMatchObject({
      comment_id: r.comment_id,
      author_kind: "agent",
      author_name: "agent",
      body_md: "Because **reasons**.",
    });
    // The session was recorded, so the comment names it.
    expect(
      query<Json>(
        dataDir,
        "SELECT session_id FROM comments WHERE id = ?",
        r.comment_id,
      ),
    ).toEqual([{ session_id: "sess-cli" }]);

    // A body file on disk works too, and --resolve resolves.
    const bodyFile = join(home, "body.md");
    writeFileSync(bodyFile, "Fixed in the next commit.");
    const resolved = pgOk(env, [
      "reply",
      t.thread_id,
      "--body-file",
      bodyFile,
      "--resolve",
    ]);
    expect(resolved.status).toBe("resolved");

    const reopened = pgOk(env, ["unresolve", t.thread_id]);
    expect(reopened).toEqual({ thread_id: t.thread_id, status: "open" });
    const closed = pgOk(
      env,
      ["resolve", t.thread_id, "--body-file", "-"],
      "Done.",
    );
    expect(closed).toEqual({
      thread_id: t.thread_id,
      status: "resolved",
      resolved_by: { kind: "agent", name: "agent", at: expect.any(String) },
    });
    expect(closed.resolved_by.at).toMatch(RFC3339);
    const after = pgOk(env, ["thread", t.thread_id]);
    expect(after.comments.at(-1).body_md).toBe("Done.");

    // Unreadable bodies are errors, as JSON on stdout.
    const missing = pg(env, [
      "reply",
      t.thread_id,
      "--body-file",
      join(home, "nope.md"),
    ]);
    expect(missing.exitCode).toBe(1);
    expect(missing.json.error.code).toBe("not_found");
    const empty = pg(env, ["reply", t.thread_id, "--body-file", "-"], "");
    expect(empty.exitCode).toBe(1);
    expect(empty.json.error.code).toBe("conflict");
  });

  test("comment creates a question", async () => {
    const { env, home } = world();
    const { reviewId, diffId } = liveReview(env, home);
    const r = pgOk(
      env,
      [
        "comment",
        reviewId,
        "--kind",
        "question",
        "--path",
        "a.txt",
        "--line",
        "5",
        "--body-file",
        "-",
      ],
      "Should L5 stay uppercase?\n",
    );
    expect(r).toEqual({
      thread_id: expect.any(String),
      diff_id: diffId,
      iteration: 1,
    });
    const t = pgOk(env, ["thread", r.thread_id]);
    expect(t).toMatchObject({
      kind: "question",
      subject: "line",
      path: "a.txt",
      side: "new",
      line: 5,
      created_by: { kind: "agent", name: "agent" },
    });
    expect(t.comments[0].body_md).toBe("Should L5 stay uppercase?");

    // A review-level note, and an anchor the api rejects.
    const note = pgOk(
      env,
      ["comment", reviewId, "--kind", "note", "--body-file", "-"],
      "Overall: small change.",
    );
    expect(pgOk(env, ["thread", note.thread_id]).subject).toBe("review");
    const bad = pg(
      env,
      [
        "comment",
        reviewId,
        "--kind",
        "note",
        "--path",
        "a.txt",
        "--line",
        "99",
        "--body-file",
        "-",
      ],
      "Out of range.",
    );
    expect(bad.exitCode).toBe(1);
    expect(bad.json.error.code).toBe("invalid_anchor");
  });

  test("rereview sets status", async () => {
    const { env, home, dataDir } = world();
    const { path, reviewId } = liveReview(env, home);
    debug(env, [
      "human-submit",
      "--review",
      reviewId,
      "--verdict",
      "request-changes",
    ]);
    writeFileSync(join(path, "a.txt"), `${TEN.join("\n")}\nl11\n`);
    const r = pgOk(
      env,
      ["rereview", reviewId, "--summary-file", "-"],
      "Reverted L5 and added l11.\n",
    );
    expect(r).toEqual({
      review_id: reviewId,
      status: "rereview_requested",
      iteration: 2,
      diff_id: expect.stringMatching(/^[0-9a-f]{64}$/),
    });
    const listed = pgOk(env, ["reviews"]).reviews as Json[];
    expect(listed[0]).toMatchObject({
      review_id: reviewId,
      status: "rereview_requested",
      rereview: { summary: "Reverted L5 and added l11." },
    });
    expect(existsSync(join(dataDir, "launched.log"))).toBe(false);

    const blank = pg(
      env,
      ["rereview", reviewId, "--summary-file", "-"],
      "   \n",
    );
    expect(blank.exitCode).toBe(1);
    expect(blank.json.error.code).toBe("conflict");
  });

  test("rereview launches a closed app in the background unless --no-open", async () => {
    const { env, home, dataDir } = world();
    const appEnv = withAppBin(env, dataDir);
    const { reviewId } = liveReview(env, home);
    debug(env, ["human-submit", "--review", reviewId]);
    pgOk(
      appEnv,
      ["--no-open", "rereview", reviewId, "--summary-file", "-"],
      "First.",
    );
    expect(existsSync(join(dataDir, "launched.log"))).toBe(false);
    pgOk(appEnv, ["rereview", reviewId, "--summary-file", "-"], "Second.");
    for (let i = 0; i < 200 && !existsSync(join(dataDir, "launched.log")); i++)
      await Bun.sleep(25);
    expect(lines(join(dataDir, "launched.log"))).toEqual([{ activate: "0" }]);
  });

  test("edit and delete equal their MCP twins", async () => {
    const { env, home, dataDir } = world();
    const { reviewId } = liveReview(env, home);
    const agent = { ...env, POLYGLOSS_AGENT: "claude-code" };
    const a = pgOk(
      agent,
      ["comment", reviewId, "--kind", "note", "--body-file", "-"],
      "First note.",
    );
    const b = pgOk(
      agent,
      ["comment", reviewId, "--kind", "note", "--body-file", "-"],
      "Second note.",
    );
    const rootOf = (thread: string) =>
      pgOk(env, ["thread", thread]).comments[0].comment_id as string;
    const [ca, cb] = [rootOf(a.thread_id), rootOf(b.thread_id)];

    const cliEdit = pgOk(
      agent,
      ["edit", ca, "--body-file", "-"],
      "First note, edited.",
    );
    const mcpEdit = await withMcp(env, async (call) =>
      ok(
        await call("edit_comment", {
          comment_id: cb,
          body_md: "Second note, edited.",
        }),
      ),
    );
    expect(Object.keys(cliEdit).sort()).toEqual(Object.keys(mcpEdit).sort());
    expect(cliEdit).toEqual({ comment_id: ca, edited_at: expect.any(String) });
    expect(cliEdit.edited_at).toMatch(RFC3339);
    expect(pgOk(env, ["thread", a.thread_id]).comments[0].body_md).toBe(
      "First note, edited.",
    );

    // Someone else's comment (another author name) is forbidden, as in MCP.
    const forbidden = pg(env, ["edit", ca, "--body-file", "-"], "Hijack.");
    expect(forbidden.exitCode).toBe(1);
    expect(forbidden.json.error.code).toBe("forbidden");
    expect(pg(env, ["delete", ca]).json.error.code).toBe("forbidden");

    const cliDelete = pgOk(agent, ["delete", ca]);
    const mcpDelete = await withMcp(env, async (call) =>
      ok(await call("delete_comment", { comment_id: cb })),
    );
    expect(cliDelete).toEqual({
      comment_id: ca,
      deleted: true,
      placeholder: false,
    });
    expect(mcpDelete).toEqual({ ...cliDelete, comment_id: cb });
    expect(
      query<Json>(
        dataDir,
        "SELECT count(*) AS n FROM events WHERE kind IN ('comment.edited', 'comment.deleted')",
      )[0]!.n,
    ).toBe(4);
  });

  test("--agent sets the author name", async () => {
    const { env, home } = world();
    const { reviewId } = liveReview(env, home);
    const authorOf = (args: string[], e: Env) => {
      const r = pgOk(
        e,
        ["comment", reviewId, "--kind", "note", "--body-file", "-", ...args],
        "Hi.",
      );
      return pgOk(env, ["thread", r.thread_id]).comments[0]
        .author_name as string;
    };
    expect(authorOf(["--agent", "codex"], env)).toBe("codex");
    expect(
      authorOf(["--agent", "codex"], { ...env, POLYGLOSS_AGENT: "cursor" }),
    ).toBe("codex");
    expect(authorOf([], { ...env, POLYGLOSS_AGENT: "cursor" })).toBe("cursor");
    expect(authorOf([], { ...env, POLYGLOSS_AGENT: "" })).toBe("agent");
    expect(authorOf([], env)).toBe("agent");
  });
});

describe("focus", () => {
  test("focus with --no-open reports unavailable", async () => {
    const { env, home, dataDir } = world();
    const appEnv = withAppBin(env, dataDir);
    const { reviewId, diffId } = liveReview(env, home);
    expect(
      pgOk(appEnv, [
        "--no-open",
        "focus",
        reviewId,
        "--path",
        "a.txt",
        "--line",
        "5",
      ]),
    ).toEqual({ status: "unavailable" });
    expect(pgOk(appEnv, ["focus", diffId.slice(0, 10), "--no-open"])).toEqual({
      status: "unavailable",
    });
    expect(existsSync(join(dataDir, "launched.log"))).toBe(false);
    // The ids are still checked.
    const unknown = pg(appEnv, ["--no-open", "focus", "no-such-review"]);
    expect(unknown.exitCode).toBe(1);
    expect(unknown.json.error.code).toBe("not_found");
  });

  test("focus launches the app and accepts a diff id prefix", async () => {
    const { env, home, dataDir } = world();
    const appEnv = withAppBin(env, dataDir);
    const { reviewId, diffId } = liveReview(env, home);
    expect(
      pgOk(appEnv, [
        "focus",
        diffId.slice(0, 12),
        "--path",
        "a.txt",
        "--side",
        "new",
        "--line",
        "5",
      ]),
    ).toEqual({ status: "launched" });
    expect(pgOk(appEnv, ["focus", reviewId])).toEqual({ status: "focused" });
    const focus = lines(join(dataDir, "ops.log")).filter(
      (op) => op.op === "focus",
    );
    expect(focus).toEqual([
      {
        v: 1,
        id: expect.any(Number),
        op: "focus",
        diff_id: diffId,
        path: "a.txt",
        side: "new",
        line: 5,
      },
      { v: 1, id: expect.any(Number), op: "focus", review_id: reviewId },
    ]);
  });
});
