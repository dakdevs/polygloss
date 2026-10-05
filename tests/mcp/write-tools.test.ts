// MCP write tools (T4.6, design §15.2): `open_diff`, `create_comment`,
// `reply`, `resolve`, `unresolve`, `edit_comment`, `delete_comment`,
// `request_rereview` and `focus`, driven through a real `polygloss-cli mcp`.
// The human side is played by the hidden `polygloss-cli debug` commands; the
// app by a fake that binds the sandbox socket and records every op (and, when
// launched through POLYGLOSS_APP_BIN, how it was launched). Nothing here ever
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

/**
 * A sandbox whose data dir is short enough for the socket path (104 bytes),
 * in test mode with no app override: an accidental launch fails at once.
 */
function world(session = "sess-me"): {
  env: Env;
  home: string;
  dataDir: string;
} {
  const sandbox = makeSandbox();
  const dataDir = realpathSync(mkdtempSync("/tmp/pgw-"));
  cleanups.push(() => {
    sandbox.cleanup();
    rmSync(dataDir, { recursive: true, force: true });
  });
  return {
    env: {
      ...sandbox.env,
      POLYGLOSS_DATA_DIR: dataDir,
      POLYGLOSS_TEST: "1",
      CLAUDE_CODE_SESSION_ID: session,
    },
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

/** A repo on `main` with `a.txt` (ten lines) and `b.txt`, plus a `feature`
 * branch that changes line 5 of `a.txt`; the worktree has line 5 edited. */
function repo(env: Env, home: string, name = "repo"): string {
  const path = join(home, name);
  mkdirSync(path, { recursive: true });
  git(env, path, ["init", "-q", "-b", "main"]);
  const write = (file: string, lines: string[]) =>
    writeFileSync(join(path, file), `${lines.join("\n")}\n`);
  write("a.txt", TEN);
  write("b.txt", ["b1", "b2"]);
  git(env, path, ["add", "."]);
  git(env, path, ["commit", "-q", "-m", "init"]);
  git(env, path, ["checkout", "-q", "-b", "feature"]);
  write(
    "a.txt",
    TEN.map((l) => (l === "l5" ? "F5" : l)),
  );
  git(env, path, ["commit", "-q", "-am", "feature"]);
  git(env, path, ["checkout", "-q", "main"]);
  write(
    "a.txt",
    TEN.map((l) => (l === "l5" ? "L5" : l)),
  );
  return realpathSync(path);
}

function debug(env: Env, args: string[]): Json {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
  if (r.exitCode !== 0)
    throw new Error(`debug ${args.join(" ")}: ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString()) as Json;
}

function query<T>(dataDir: string, sql: string, ...params: any[]): T[] {
  const db = new Database(join(dataDir, "polygloss.db"), { readwrite: true });
  try {
    return db.query(sql).all(...params) as T[];
  } finally {
    db.close();
  }
}

type Call = (name: string, args?: Json) => Promise<Json>;

async function withMcp<T>(
  env: Env,
  f: (call: Call) => Promise<T>,
  opts: { clientName?: string; viaShell?: boolean } = {},
): Promise<T> {
  const mcp = await connectMcp({
    env,
    clientName: opts.clientName ?? "claude-code",
    viaShell: opts.viaShell,
  });
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

/** `open_diff` of the live worktree since HEAD, without showing it. */
async function openLive(call: Call, path: string, extra: Json = {}) {
  return ok(
    await call("open_diff", {
      repo: path,
      source: { kind: "live", since: "HEAD" },
      show: false,
      ...extra,
    }),
  );
}

/** A fake app: records its launch (argv and POLYGLOSS_LAUNCH_ACTIVATE) and
 * every request on `<dataDir>/polygloss.sock`, answering each with ok. */
function fakeAppSource(): string {
  return `
import { appendFileSync } from "node:fs";
const dir = process.env.POLYGLOSS_DATA_DIR;
appendFileSync(dir + "/launched.log", JSON.stringify({
  argv: process.argv.slice(2),
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
setTimeout(() => process.exit(0), Number(process.env.FAKE_APP_LIFETIME_MS ?? 20000));
`;
}

/** Installs the fake app as POLYGLOSS_APP_BIN. */
function withAppBin(env: Env, dataDir: string): Env {
  const bin = join(dataDir, "fake-polygloss");
  writeFileSync(bin, `#!${process.execPath}\n${fakeAppSource()}`);
  chmodSync(bin, 0o755);
  return { ...env, POLYGLOSS_APP_BIN: bin, FAKE_APP_LIFETIME_MS: "15000" };
}

/** Starts the fake app by hand (already running before any tool call). */
async function startFakeApp(env: Env, dataDir: string) {
  const script = join(dataDir, "fake-app.ts");
  writeFileSync(script, fakeAppSource());
  const app = Bun.spawn([process.execPath, script], {
    env: { ...env, FAKE_APP_LIFETIME_MS: "30000" },
    stdout: "ignore",
    stderr: "inherit",
  });
  const sock = join(dataDir, "polygloss.sock");
  for (let i = 0; i < 200 && !existsSync(sock); i++) await Bun.sleep(25);
  expect(existsSync(sock)).toBe(true);
  return {
    stop: async () => {
      app.kill();
      await app.exited;
    },
  };
}

function lines(file: string): Json[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf8")
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((l) => JSON.parse(l) as Json);
}

async function waitFor<T>(f: () => T | undefined, what: string): Promise<T> {
  for (let i = 0; i < 400; i++) {
    const v = f();
    if (v !== undefined) return v;
    await Bun.sleep(25);
  }
  throw new Error(`timed out waiting for ${what}`);
}

const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

describe("open_diff", () => {
  test("open_diff live pins snapshot and returns url", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = await openLive(call, path);
      expect(r.iteration).toBe(1);
      expect(r.diff_id).toMatch(/^[0-9a-f]{64}$/);
      expect(r.url).toBe(`polygloss://diff/${r.diff_id}`);
      expect(r.review_key).toContain("#since=HEAD");
      expect(r.stats).toEqual({ files: 1, additions: 1, deletions: 1 });
      expect(r.files).toEqual([
        { path: "a.txt", status: "modified", additions: 1, deletions: 1 },
      ]);
      expect(r.files_truncated).toBe(false);
      expect(r.base.commit).toBe(git(env, path, ["rev-parse", "HEAD"]));
      expect(r.head.commit).toBeUndefined();

      const its = query<Json>(
        dataDir,
        "SELECT pinned_by, snapshot_ref, diff_id FROM iterations",
      );
      expect(its).toEqual([
        {
          pinned_by: "agent",
          snapshot_ref: expect.stringMatching(/^refs\/polygloss\/snapshots\//),
          diff_id: r.diff_id,
        },
      ]);
      // The snapshot ref is in the user's repo; HEAD and the index are not.
      expect(git(env, path, ["for-each-ref", "refs/polygloss/"])).toContain(
        its[0]!.snapshot_ref,
      );
      expect(git(env, path, ["status", "--porcelain"])).toBe("M a.txt");

      // The review is readable at once, with the same ids.
      const listed = ok(await call("list_reviews", {}));
      expect(listed.reviews[0].review_id).toBe(r.review_id);
      expect(listed.reviews[0].latest_diff_id).toBe(r.diff_id);
    });
  });

  test("open_diff show false skips app", async () => {
    const { env, home, dataDir } = world();
    const appEnv = withAppBin(env, dataDir);
    const path = repo(env, home);
    await withMcp(appEnv, async (call) => {
      const r = await openLive(call, path);
      expect(r.app).toBe("skipped");
    });
    expect(existsSync(join(dataDir, "launched.log"))).toBe(false);
  });

  test("open_diff app unavailable is reported not thrown", async () => {
    // Test mode without POLYGLOSS_APP_BIN: the launch is refused at once.
    const { env, home } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = ok(await call("open_diff", { repo: path }));
      expect(r.app).toBe("unavailable");
      expect(r.review_id).toBeString();
      expect(r.iteration).toBe(1);
    });
  });

  test("open_diff shows the review in a running app in the background", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    const app = await startFakeApp(env, dataDir);
    try {
      await withMcp(env, async (call) => {
        const r = ok(await call("open_diff", { repo: path }));
        expect(r.app).toBe("opened");
        const opens = lines(join(dataDir, "ops.log")).filter(
          (op) => op.op === "open",
        );
        expect(opens).toEqual([
          {
            v: 1,
            id: expect.any(Number),
            op: "open",
            review_id: r.review_id,
            activate: false,
          },
        ]);
      });
    } finally {
      await app.stop();
    }
  });

  test("open_diff assigns review to caller latest opener wins", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    const mine = { ...env, CLAUDE_CODE_SESSION_ID: "sess-a" };
    const theirs = { ...env, CLAUDE_CODE_SESSION_ID: "sess-b" };
    const assigned = () =>
      query<Json>(
        dataDir,
        "SELECT session_id, assigned_by FROM review_assignments",
      );
    const assignedToMe = (call: Call) =>
      call("list_reviews", { assigned: "me" }).then(
        (r) => ok(r).reviews.length as number,
      );

    // Each server under its own shell: distinct owner pids, unlinked sessions.
    await withMcp(
      mine,
      async (callA) => {
        const r = await openLive(callA, path);
        expect(assigned()).toEqual([
          { session_id: "sess-a", assigned_by: "open_diff" },
        ]);
        await withMcp(
          theirs,
          async (callB) => {
            const again = await openLive(callB, path);
            expect(again.review_id).toBe(r.review_id);
            expect(assigned()).toEqual([
              { session_id: "sess-b", assigned_by: "open_diff" },
            ]);
            expect(await assignedToMe(callA)).toBe(0);
            expect(await assignedToMe(callB)).toBe(1);
            // assign: false leaves the assignment alone.
            await openLive(callA, path, { assign: false });
            expect(assigned()[0]!.session_id).toBe("sess-b");
          },
          { viaShell: true },
        );
        await openLive(callA, path);
        expect(assigned()[0]!.session_id).toBe("sess-a");
        expect(await assignedToMe(callA)).toBe(1);
      },
      { viaShell: true },
    );
  });

  test("open_diff compare with label", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = ok(
        await call("open_diff", {
          repo: path,
          source: { kind: "compare", base: "main", head: "feature" },
          label: "PR #7",
          show: false,
        }),
      );
      expect(r.review_key).toBe("compare:refs/heads/main...refs/heads/feature");
      expect(r.base.rev).toBe("refs/heads/main");
      expect(r.head).toEqual({
        rev: "refs/heads/feature",
        commit: git(env, path, ["rev-parse", "feature"]),
        tree: git(env, path, ["rev-parse", "feature^{tree}"]),
      });
      expect(r.files).toEqual([
        { path: "a.txt", status: "modified", additions: 1, deletions: 1 },
      ]);
      const reviews = ok(await call("list_reviews", {})).reviews as Json[];
      expect(reviews.map((x) => [x.kind, x.label])).toEqual([
        ["compare", "PR #7"],
      ]);
      expect(
        query<Json>(dataDir, "SELECT pinned_by FROM iterations")[0]!.pinned_by,
      ).toBe("open");
    });
  });

  test("open_diff returns categories per file and in stats", async () => {
    const { env, home } = world();
    const path = join(home, "cats");
    mkdirSync(path, { recursive: true });
    git(env, path, ["init", "-q", "-b", "main"]);
    git(env, path, ["commit", "-q", "--allow-empty", "-m", "init"]);
    const write = (file: string, text: string) => {
      mkdirSync(join(path, file, ".."), { recursive: true });
      writeFileSync(join(path, file), text);
    };
    write("src/a.ts", "a\n");
    write("src/a.test.ts", "t1\nt2\nt3\nt4\n");
    write("Cargo.lock", "l1\nl2\nl3\n");
    write("docs/x.md", "# x\nbody\n");
    // The sandbox's settings.json turns Docs on.
    const config = join(env.XDG_CONFIG_HOME!, "polygloss");
    mkdirSync(config, { recursive: true });
    writeFileSync(
      join(config, "settings.json"),
      '{ "categories": { "docs": { "enabled": true } } }',
    );

    await withMcp(env, async (call) => {
      const r = await openLive(call, realpathSync(path));
      // Git order, unchanged; uncategorized files carry no `category`.
      expect(r.files).toEqual([
        {
          path: "Cargo.lock",
          status: "added",
          additions: 3,
          deletions: 0,
          category: "generated",
        },
        {
          path: "docs/x.md",
          status: "added",
          additions: 2,
          deletions: 0,
          category: "docs",
        },
        {
          path: "src/a.test.ts",
          status: "added",
          additions: 4,
          deletions: 0,
          category: "tests",
        },
        { path: "src/a.ts", status: "added", additions: 1, deletions: 0 },
      ]);
      // `stats` keep counting categorized files.
      expect(r.stats).toEqual({
        files: 4,
        additions: 10,
        deletions: 0,
        categories: {
          docs: { files: 1, additions: 2, deletions: 0 },
          generated: { files: 1, additions: 3, deletions: 0 },
          tests: { files: 1, additions: 4, deletions: 0 },
        },
      });
    });
  });
});

describe("create_comment", () => {
  test("create_comment note is published and visible", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = await openLive(call, path);
      // The agent keeps editing, then explains a line: the comment pins the
      // worktree as it is now.
      writeFileSync(join(path, "b.txt"), "b1\nB2\n");
      const c = ok(
        await call("create_comment", {
          review_id: r.review_id,
          kind: "note",
          body_md: "Renamed for clarity.",
          anchor: { path: "b.txt", side: "new", line: 2 },
        }),
      );
      expect(c.iteration).toBe(2);
      expect(c.diff_id).not.toBe(r.diff_id);
      expect(
        query<Json>(dataDir, "SELECT pinned_by FROM iterations ORDER BY seq"),
      ).toEqual([{ pinned_by: "agent" }, { pinned_by: "agent" }]);

      // Visible at once to agents (never a draft), with its author.
      const threads = ok(await call("list_threads", { review_id: r.review_id }))
        .threads as Json[];
      expect(threads).toHaveLength(1);
      expect(threads[0]).toMatchObject({
        thread_id: c.thread_id,
        kind: "note",
        subject: "line",
        path: "b.txt",
        side: "new",
        line: 2,
        position: { state: "exact", start_line: 2, line: 2 },
        status: "open",
        created_by: { kind: "agent", name: "claude-code" },
      });
      const events = query<Json>(
        dataDir,
        "SELECT kind, actor_name FROM events WHERE thread_id = ?",
        c.thread_id,
      );
      expect(events).toEqual([
        { kind: "thread.created", actor_name: "claude-code" },
      ]);
      const published = query<Json>(
        dataDir,
        "SELECT published_at IS NOT NULL AS published FROM comments",
      );
      expect(published).toEqual([{ published: 1 }]);

      // A question by diff id prefix on the file.
      const q = ok(
        await call("create_comment", {
          diff_id: c.diff_id.slice(0, 12),
          kind: "question",
          body_md: "Keep both?",
          anchor: { path: "a.txt" },
        }),
      );
      const got = ok(await call("get_thread", { thread_id: q.thread_id }));
      expect(got).toMatchObject({ kind: "question", subject: "file" });
    });
  });

  test("create_comment invalid anchor", async () => {
    const { env, home } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = await openLive(call, path);
      const bad = async (anchor: Json) =>
        (
          await call("create_comment", {
            review_id: r.review_id,
            kind: "note",
            body_md: "x",
            anchor,
          })
        ).error;
      for (const anchor of [
        { path: "missing.txt", line: 1 },
        { path: "a.txt", side: "new", line: 11 },
        { path: "a.txt", side: "new", line: 3, start_line: 4 },
        { path: "a.txt", side: "new", line: 0 },
        { path: "a.txt", side: "old" },
      ]) {
        const err = await bad(anchor);
        expect(err?.code).toBe("invalid_anchor");
        expect(err?.message).toBeString();
      }
      expect(
        (await call("create_comment", { kind: "note", body_md: "x" })).error
          .code,
      ).toBe("conflict");
    });
  });

  test("create_comment cap exceeded after 50", async () => {
    const { env, home } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = await openLive(call, path);
      const note = (i: number) =>
        call("create_comment", {
          diff_id: r.diff_id,
          kind: "note",
          body_md: `note ${i}`,
        });
      for (let i = 0; i < 50; i++) ok(await note(i));
      const over = await note(50);
      expect(over.error.code).toBe("cap_exceeded");
      // Replies do not count.
      const threads = ok(
        await call("list_threads", { review_id: r.review_id, limit: 1 }),
      ).threads as Json[];
      ok(
        await call("reply", {
          thread_id: threads[0]!.thread_id,
          body_md: "still fine",
        }),
      );
    });
  });
});

describe("reply, resolve, unresolve", () => {
  test("reply and resolve record the clientInfo name", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    const r = await withMcp(env, async (call) => openLive(call, path));
    // A human thread, submitted.
    const human = debug(env, [
      "human-comment",
      "--review",
      r.review_id,
      "--body",
      "Rename this?",
      "--path",
      "a.txt",
      "--line",
      "5",
    ]);
    debug(env, ["human-submit", "--review", r.review_id]);

    await withMcp(
      env,
      async (call) => {
        const reply = ok(
          await call("reply", {
            thread_id: human.thread_id,
            body_md: "Renamed.",
          }),
        );
        expect(reply).toEqual({
          comment_id: expect.any(String),
          thread_id: human.thread_id,
          status: "open",
        });
        const resolved = ok(
          await call("reply", {
            thread_id: human.thread_id,
            body_md: "And tested.",
            resolve: true,
          }),
        );
        expect(resolved.status).toBe("resolved");
        const t = ok(await call("get_thread", { thread_id: human.thread_id }));
        expect(
          (t.comments as Json[]).map((c) => [c.author_kind, c.author_name]),
        ).toEqual([
          ["human", "you"],
          ["agent", "my-agent"],
          ["agent", "my-agent"],
        ]);
        expect(t.resolved_by).toMatchObject({
          kind: "agent",
          name: "my-agent",
          at: expect.stringMatching(RFC3339),
        });
        const again = ok(await call("resolve", { thread_id: human.thread_id }));
        expect(again).toEqual({
          thread_id: human.thread_id,
          status: "resolved",
          resolved_by: {
            kind: "agent",
            name: "my-agent",
            at: expect.stringMatching(RFC3339),
          },
        });
      },
      { clientName: "my-agent" },
    );
    const kinds = query<Json>(
      dataDir,
      "SELECT kind, actor_name FROM events WHERE thread_id = ? ORDER BY seq",
      human.thread_id,
    ).map((e) => `${e.kind}:${e.actor_name}`);
    expect(kinds).toContain("comment.created:my-agent");
    expect(kinds).toContain("thread.resolved:my-agent");
  });

  test("unresolve reopens", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = await openLive(call, path);
      const c = ok(
        await call("create_comment", {
          review_id: r.review_id,
          kind: "question",
          body_md: "Which one?",
        }),
      );
      const closed = ok(
        await call("resolve", {
          thread_id: c.thread_id,
          body_md: "Answered in chat.",
        }),
      );
      expect(closed.status).toBe("resolved");
      const open = ok(await call("unresolve", { thread_id: c.thread_id }));
      expect(open).toEqual({ thread_id: c.thread_id, status: "open" });
      const t = ok(await call("get_thread", { thread_id: c.thread_id }));
      expect(t.status).toBe("open");
      expect(t.resolved_by).toBeUndefined();
      expect(t.comment_count).toBe(2);
      expect(
        query<Json>(
          dataDir,
          "SELECT kind FROM events WHERE thread_id = ? ORDER BY seq",
          c.thread_id,
        ).map((e) => e.kind),
      ).toEqual([
        "thread.created",
        "comment.created",
        "thread.resolved",
        "thread.unresolved",
      ]);
      expect(
        (await call("unresolve", { thread_id: "no-such-thread" })).error.code,
      ).toBe("not_found");
    });
  });
});

describe("edit_comment and delete_comment", () => {
  test("edit_comment edits own comment and emits comment.edited", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    await withMcp(env, async (call) => {
      const r = await openLive(call, path);
      const c = ok(
        await call("create_comment", {
          review_id: r.review_id,
          kind: "note",
          body_md: "First draft of the note.",
        }),
      );
      const t = ok(await call("get_thread", { thread_id: c.thread_id }));
      const commentId = t.comments[0].comment_id as string;
      const edited = ok(
        await call("edit_comment", {
          comment_id: commentId,
          body_md: "The note, edited.",
        }),
      );
      expect(edited).toEqual({
        comment_id: commentId,
        edited_at: expect.stringMatching(RFC3339),
      });
      const after = ok(await call("get_thread", { thread_id: c.thread_id }));
      expect(after.comments[0]).toMatchObject({
        body_md: "The note, edited.",
        edited_at: edited.edited_at,
      });
      expect(
        query<Json>(
          dataDir,
          "SELECT kind, comment_id FROM events WHERE kind = 'comment.edited'",
        ),
      ).toEqual([{ kind: "comment.edited", comment_id: commentId }]);
    });
    // Another client is not the author (OQ-30).
    const threads = query<Json>(dataDir, "SELECT id FROM comments");
    await withMcp(
      env,
      async (call) => {
        const err = (
          await call("edit_comment", {
            comment_id: threads[0]!.id,
            body_md: "Not mine.",
          })
        ).error;
        expect(err.code).toBe("forbidden");
      },
      { clientName: "other-agent" },
    );
  });

  test("edit_comment on a human comment is forbidden", async () => {
    const { env, home } = world();
    const path = repo(env, home);
    const r = await withMcp(env, async (call) => openLive(call, path));
    const human = debug(env, [
      "human-comment",
      "--review",
      r.review_id,
      "--body",
      "Human words.",
    ]);
    debug(env, ["human-submit", "--review", r.review_id]);
    await withMcp(env, async (call) => {
      const t = ok(await call("get_thread", { thread_id: human.thread_id }));
      const commentId = t.comments[0].comment_id;
      for (const [tool, args] of [
        ["edit_comment", { comment_id: commentId, body_md: "Agent words." }],
        ["delete_comment", { comment_id: commentId }],
      ] as const) {
        const err = (await call(tool, args)).error;
        expect(err.code).toBe("forbidden");
      }
      const unchanged = ok(
        await call("get_thread", { thread_id: human.thread_id }),
      );
      expect(unchanged.comments[0].body_md).toBe("Human words.");
    });
  });

  test("delete_comment with replies leaves a placeholder", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    const r = await withMcp(env, async (call) => openLive(call, path));
    const note = await withMcp(env, async (call) =>
      ok(
        await call("create_comment", {
          review_id: r.review_id,
          kind: "note",
          body_md: "Explains the change.",
          anchor: { path: "a.txt", line: 5 },
        }),
      ),
    );
    debug(env, [
      "human-comment",
      "--review",
      r.review_id,
      "--reply-to",
      note.thread_id,
      "--body",
      "Thanks.",
    ]);
    debug(env, ["human-submit", "--review", r.review_id]);
    await withMcp(env, async (call) => {
      const t = ok(await call("get_thread", { thread_id: note.thread_id }));
      const root = t.comments[0].comment_id as string;
      const deleted = ok(await call("delete_comment", { comment_id: root }));
      expect(deleted).toEqual({
        comment_id: root,
        deleted: true,
        placeholder: true,
      });
      const after = ok(await call("get_thread", { thread_id: note.thread_id }));
      expect(after.comments[0]).toMatchObject({
        comment_id: root,
        deleted: true,
        body_md: "",
      });
      expect(after.comments[1].body_md).toBe("Thanks.");
      expect(after.comment_count).toBe(1);

      // A reply without replies after it simply goes.
      const mine = ok(
        await call("reply", { thread_id: note.thread_id, body_md: "Bye." }),
      );
      expect(
        ok(await call("delete_comment", { comment_id: mine.comment_id })),
      ).toEqual({
        comment_id: mine.comment_id,
        deleted: true,
        placeholder: false,
      });
    });
    expect(
      query<Json>(
        dataDir,
        "SELECT count(*) AS n FROM events WHERE kind = 'comment.deleted'",
      )[0]!.n,
    ).toBe(2);
  });
});

describe("request_rereview", () => {
  test("request_rereview sets status and new live iteration", async () => {
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    const r = await withMcp(env, async (call) => openLive(call, path));
    debug(env, [
      "human-submit",
      "--review",
      r.review_id,
      "--verdict",
      "request-changes",
      "--summary",
      "Fix b too.",
    ]);
    writeFileSync(join(path, "b.txt"), "b1\nB2\n");
    // Muted, so the closed app is not launched to notify.
    query(dataDir, "UPDATE reviews SET muted = 1 WHERE id = ?", r.review_id);
    await withMcp(env, async (call) => {
      const rr = ok(
        await call("request_rereview", {
          review_id: r.review_id,
          summary_md: "Fixed b as asked.",
        }),
      );
      expect(rr).toEqual({
        review_id: r.review_id,
        status: "rereview_requested",
        iteration: 2,
        diff_id: expect.stringMatching(/^[0-9a-f]{64}$/),
      });
      const listed = ok(await call("list_reviews", {})).reviews[0];
      expect(listed.status).toBe("rereview_requested");
      expect(listed.rereview.summary).toBe("Fixed b as asked.");
      expect(listed.latest_diff_id).toBe(rr.diff_id);
      expect(
        (
          await call("request_rereview", {
            review_id: r.review_id,
            summary_md: " ",
          })
        ).error.code,
      ).toBe("conflict");
    });
    // 1: open_diff (the submission reviewed it), 2: the worktree now.
    expect(
      query<Json>(
        dataDir,
        "SELECT seq, pinned_by FROM iterations ORDER BY seq",
      ),
    ).toEqual([
      { seq: 1, pinned_by: "agent" },
      { seq: 2, pinned_by: "rereview" },
    ]);
    expect(
      query<Json>(
        dataDir,
        "SELECT kind FROM events WHERE kind = 'review.rereview_requested'",
      ),
    ).toHaveLength(1);
  });

  test("request_rereview launches the app hidden when it is not running", async () => {
    const { env, home, dataDir } = world();
    const appEnv = withAppBin(env, dataDir);
    const path = repo(env, home);
    await withMcp(appEnv, async (call) => {
      const r = await openLive(call, path);
      expect(existsSync(join(dataDir, "launched.log"))).toBe(false);
      const started = Date.now();
      ok(
        await call("request_rereview", {
          review_id: r.review_id,
          summary_md: "Please look again.",
        }),
      );
      // The call does not wait for the app.
      expect(Date.now() - started).toBeLessThan(10_000);
      const launched = await waitFor(
        () => lines(join(dataDir, "launched.log"))[0],
        "the fake app's launch",
      );
      expect(launched).toEqual({
        argv: [`polygloss://review/${r.review_id}`],
        activate: "0",
      });
    });
  });
});

describe("focus", () => {
  test("focus launches app through override", async () => {
    const { env, home, dataDir } = world();
    const appEnv = withAppBin(env, dataDir);
    const path = repo(env, home);
    await withMcp(appEnv, async (call) => {
      const r = await openLive(call, path);
      const res = ok(
        await call("focus", {
          review_id: r.review_id,
          path: "a.txt",
          side: "new",
          line: 5,
        }),
      );
      expect(res).toEqual({ status: "launched" });
      expect(lines(join(dataDir, "launched.log"))).toEqual([
        { argv: [], activate: "0" },
      ]);
      const focus = lines(join(dataDir, "ops.log")).filter(
        (op) => op.op === "focus",
      );
      expect(focus).toEqual([
        {
          v: 1,
          id: expect.any(Number),
          op: "focus",
          review_id: r.review_id,
          path: "a.txt",
          side: "new",
          line: 5,
        },
      ]);
      // Running now: focused without another launch.
      const again = ok(
        await call("focus", { diff_id: r.diff_id.slice(0, 10), path: "a.txt" }),
      );
      expect(again).toEqual({ status: "focused" });
      expect(lines(join(dataDir, "launched.log"))).toHaveLength(1);
      expect(
        lines(join(dataDir, "ops.log"))
          .filter((op) => op.op === "focus")
          .at(-1),
      ).toMatchObject({ diff_id: r.diff_id, path: "a.txt" });
      expect((await call("focus", {})).error.code).toBe("conflict");
    });
  });
});

describe("nudges", () => {
  test("write tools nudge a running app with store_changed and never launch it", async () => {
    // With no app at all, no write launches one (the override would record it).
    const quiet = world();
    const quietEnv = withAppBin(quiet.env, quiet.dataDir);
    const quietPath = repo(quiet.env, quiet.home);
    await withMcp(quietEnv, async (call) => {
      await runEveryNudgeOnlyWrite(call, quietPath);
    });
    expect(existsSync(join(quiet.dataDir, "launched.log"))).toBe(false);

    // A running app gets exactly one store_changed per write, seqs rising.
    const { env, home, dataDir } = world();
    const path = repo(env, home);
    const app = await startFakeApp(env, dataDir);
    try {
      await withMcp(env, async (call) => {
        const writes = await runEveryNudgeOnlyWrite(call, path);
        ok(
          await call("request_rereview", {
            review_id: writes.reviewId,
            summary_md: "Done.",
          }),
        );
        const ops = lines(join(dataDir, "ops.log"));
        const nudges = ops.filter((op) => op.op === "store_changed");
        expect(nudges).toHaveLength(writes.count + 1);
        const seqs = nudges.map((n) => n.seq as number);
        expect([...seqs].sort((a, b) => a - b)).toEqual(seqs);
        expect(new Set(seqs).size).toBe(seqs.length);
        expect(ops.filter((op) => op.op !== "store_changed")).toEqual([]);
      });
      // Only the hand-started app ever ran.
      expect(lines(join(dataDir, "launched.log"))).toHaveLength(1);
    } finally {
      await app.stop();
    }
  });
});

/** Every write that only nudges: open_diff (show false), create_comment,
 * reply, resolve, unresolve, edit_comment, delete_comment. */
async function runEveryNudgeOnlyWrite(
  call: Call,
  path: string,
): Promise<{ reviewId: string; count: number }> {
  const r = await openLive(call, path);
  const c = ok(
    await call("create_comment", {
      review_id: r.review_id,
      kind: "note",
      body_md: "n",
    }),
  );
  const reply = ok(
    await call("reply", { thread_id: c.thread_id, body_md: "r" }),
  );
  ok(await call("resolve", { thread_id: c.thread_id }));
  ok(await call("unresolve", { thread_id: c.thread_id }));
  ok(
    await call("edit_comment", {
      comment_id: reply.comment_id,
      body_md: "e",
    }),
  );
  ok(await call("delete_comment", { comment_id: reply.comment_id }));
  return { reviewId: r.review_id, count: 7 };
}
