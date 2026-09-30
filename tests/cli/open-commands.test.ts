// The human CLI (design §14, T4.3): `polygloss [--since …] [<path>]`, `show`,
// `compare`, `open`, `snapshot`, the global flags, JSON output and errors.
// Every command runs with `--no-open`, or against a fake app bound at the
// sandbox socket, or with POLYGLOSS_TEST=1 and POLYGLOSS_APP_BIN pointing
// at a fake app: never the installed Polygloss on the real data dir.
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
import { makeSandbox } from "../support/sandbox";

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());
// A debug-build live open snapshots the worktree with several git calls
// (about 0.6 s each here); some tests run a handful.
setDefaultTimeout(60_000);

type Json = Record<string, any>;

function git(repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env: sandbox.env });
  if (r.exitCode !== 0)
    throw new Error(`git ${args.join(" ")}: ${r.stderr.toString()}`);
  return r.stdout.toString().trim();
}

/** A repo on `main` with one commit of `a.txt`. */
function makeRepo(name: string): string {
  const repo = join(sandbox.home, name);
  mkdirSync(repo, { recursive: true });
  git(repo, ["init", "-q", "-b", "main"]);
  writeFileSync(join(repo, "a.txt"), "one\ntwo\nthree\n");
  git(repo, ["add", "a.txt"]);
  git(repo, ["commit", "-q", "-m", "init"]);
  return realpathSync(repo);
}

function commitFile(repo: string, path: string, text: string, msg: string) {
  writeFileSync(join(repo, path), text);
  git(repo, ["add", path]);
  git(repo, ["commit", "-q", "-m", msg]);
}

function run(
  args: string[],
  opts: { env?: Record<string, string>; cwd?: string } = {},
): { json: Json; stdout: string; stderr: string; exitCode: number } {
  const r = Bun.spawnSync([cliBin(), ...args], {
    env: opts.env ?? sandbox.env,
    cwd: opts.cwd,
  });
  const stdout = r.stdout.toString();
  let json: Json = {};
  try {
    json = stdout.trim() === "" ? {} : JSON.parse(stdout);
  } catch {
    // Human output; callers check `stdout`.
  }
  return { json, stdout, stderr: r.stderr.toString(), exitCode: r.exitCode };
}

/** Runs the CLI with stdout on a pseudo-terminal (`script`). */
function runOnTty(
  args: string[],
  env: Record<string, string> = sandbox.env,
): {
  output: string;
  exitCode: number;
} {
  const r = Bun.spawnSync(["script", "-q", "/dev/null", cliBin(), ...args], {
    env,
    stdin: "ignore",
  });
  return { output: r.stdout.toString(), exitCode: r.exitCode };
}

function db(): Database {
  return new Database(join(sandbox.dataDir, "polygloss.db"), {
    readwrite: true,
  });
}

function query<T>(sql: string, ...params: any[]): T {
  const conn = db();
  try {
    return conn.query(sql).get(...params) as T;
  } finally {
    conn.close();
  }
}

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const hex64 = /^[0-9a-f]{64}$/;

describe("polygloss human commands", () => {
  test("live --no-open --json prints review and diff ids", () => {
    const repo = makeRepo("live");
    writeFileSync(join(repo, "a.txt"), "one\nTWO\nthree\n");

    const r = run(["--no-open", "--json", repo]);
    expect(r.stderr).toBe("");
    expect(r.exitCode).toBe(0);
    expect(r.json.review_id).toMatch(uuid);
    expect(r.json.diff_id).toMatch(hex64);
    expect(r.json).toMatchObject({
      kind: "live",
      review_key: `worktree:${repo}@main#since=merge-base`,
      // Unpinned: the review URL reopens the live review.
      iteration: null,
      url: `polygloss://review/${r.json.review_id}`,
      repo,
      files: 1,
      app: "skipped",
    });
    expect(r.json.base.commit).toBe(git(repo, ["rev-parse", "HEAD"]));

    // No path: the cwd. Same state, same review and diff.
    const again = run(["--no-open", "--json"], { cwd: join(repo) });
    expect(again.exitCode).toBe(0);
    expect(again.json.review_id).toBe(r.json.review_id);
    expect(again.json.diff_id).toBe(r.json.diff_id);

    // --since HEAD is its own review; --repo names the worktree too.
    const since = run(["--no-open", "--repo", repo, "--since", "HEAD"]);
    expect(since.exitCode).toBe(0);
    expect(since.json.review_key).toBe(`worktree:${repo}@main#since=HEAD`);
    expect(since.json.review_id).not.toBe(r.json.review_id);
    expect(since.json.diff_id).toBe(r.json.diff_id);
  });

  test("show resolves the first parent", () => {
    const repo = makeRepo("show");
    git(repo, ["checkout", "-q", "-b", "side"]);
    commitFile(repo, "b.txt", "side\n", "side");
    git(repo, ["checkout", "-q", "main"]);
    commitFile(repo, "a.txt", "one\n2\nthree\n", "main edit");
    const firstParent = git(repo, ["rev-parse", "HEAD"]);
    git(repo, ["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
    const merge = git(repo, ["rev-parse", "HEAD"]);

    const r = run(["show", "HEAD", "--no-open", "--repo", repo]);
    expect(r.exitCode).toBe(0);
    expect(r.json).toMatchObject({
      kind: "commit",
      review_key: `commit:${merge}`,
      iteration: 1,
      url: `polygloss://diff/${r.json.diff_id}`,
      files: 1,
      app: "skipped",
    });
    expect(r.json.base.commit).toBe(firstParent);
    expect(r.json.base.tree).toBe(git(repo, ["rev-parse", "HEAD^1^{tree}"]));
    expect(r.json.head.commit).toBe(merge);
    // Only the side branch's file changed against the first parent.
    const paths = query<{ paths: string }>(
      "SELECT group_concat(new_path) AS paths FROM file_changes WHERE diff_id = ?",
      r.json.diff_id,
    );
    expect(paths.paths).toBe("b.txt");

    // The cwd names the repo when --repo is absent.
    const fromCwd = run(["show", "HEAD~1", "--no-open"], { cwd: repo });
    expect(fromCwd.exitCode).toBe(0);
    expect(fromCwd.json.review_key).toBe(`commit:${firstParent}`);
  });

  test("compare defaults to three-dot and supports --direct", () => {
    const repo = makeRepo("compare");
    git(repo, ["checkout", "-q", "-b", "feature"]);
    commitFile(repo, "f.txt", "feature\n", "feature");
    git(repo, ["checkout", "-q", "main"]);
    commitFile(repo, "a.txt", "one\nmain\nthree\n", "main moves on");

    const threeDot = run(["compare", "main", "feature", "--no-open"], {
      cwd: repo,
    });
    expect(threeDot.exitCode).toBe(0);
    expect(threeDot.json).toMatchObject({
      kind: "compare",
      review_key: "compare:refs/heads/main...refs/heads/feature",
      iteration: 1,
      files: 1,
    });
    expect(threeDot.json.base.rev).toBe("refs/heads/main");
    expect(threeDot.json.head.commit).toBe(git(repo, ["rev-parse", "feature"]));

    const direct = run(
      ["compare", "main", "feature", "--direct", "--no-open"],
      {
        cwd: repo,
      },
    );
    expect(direct.exitCode).toBe(0);
    expect(direct.json).toMatchObject({
      review_key: "compare:refs/heads/main..refs/heads/feature",
      files: 2,
    });
    expect(direct.json.review_id).not.toBe(threeDot.json.review_id);
    expect(direct.json.diff_id).not.toBe(threeDot.json.diff_id);
  });

  test("compare --label is stored", () => {
    const repo = makeRepo("label");
    git(repo, ["checkout", "-q", "-b", "pr"]);
    commitFile(repo, "p.txt", "pr\n", "pr");
    const r = run(
      ["compare", "main", "pr", "--label", "PR #123", "--no-open"],
      { cwd: repo },
    );
    expect(r.exitCode).toBe(0);
    expect(r.json.label).toBe("PR #123");
    const row = query<{ label: string }>(
      "SELECT label FROM reviews WHERE id = ?",
      r.json.review_id,
    );
    expect(row.label).toBe("PR #123");
  });

  test("open accepts an 8-char prefix and rejects ambiguous prefixes", () => {
    const repo = makeRepo("open");
    commitFile(repo, "a.txt", "changed\n", "second");
    const shown = run(["show", "HEAD", "--no-open"], { cwd: repo });
    expect(shown.exitCode).toBe(0);
    const diffId = shown.json.diff_id as string;

    // From another directory: the store finds the repo that has the trees.
    const opened = run(["open", diffId.slice(0, 8), "--no-open"], {
      cwd: sandbox.home,
    });
    expect(opened.stderr).toBe("");
    expect(opened.exitCode).toBe(0);
    expect(opened.json).toMatchObject({
      diff_id: diffId,
      review_id: shown.json.review_id,
      url: `polygloss://diff/${diffId}`,
      repo,
      app: "skipped",
    });

    // Two stored diffs share the prefix `deadbeef`.
    const conn = db();
    try {
      for (const tail of ["1", "2"]) {
        conn
          .query(
            "INSERT INTO diffs (id, object_format, base_tree, head_tree, created_at) VALUES (?, 'sha1', ?, ?, 0)",
          )
          .run(`deadbeef${tail.repeat(56)}`, "a".repeat(40), "b".repeat(40));
      }
    } finally {
      conn.close();
    }
    const ambiguous = run(["open", "deadbeef", "--no-open"]);
    expect(ambiguous.exitCode).toBe(1);
    expect(ambiguous.json.error.code).toBe("not_found");
    expect(ambiguous.json.error.message).toContain("ambiguous");
    expect(ambiguous.json.error.message).toContain(`deadbeef${"1".repeat(56)}`);
    expect(ambiguous.json.error.message).toContain(`deadbeef${"2".repeat(56)}`);

    // Too short, or no match at all.
    for (const bad of ["deadbee", "0123456789abcdef"]) {
      const r = run(["open", bad, "--no-open"]);
      expect(r.exitCode).toBe(1);
      expect(r.json.error.code).toBe("not_found");
    }
  });

  test("non-tty stdout defaults to json", () => {
    const repo = makeRepo("tty");
    commitFile(repo, "a.txt", "tty\n", "second");
    // Piped stdout, no --json: JSON.
    const piped = run(["show", "HEAD", "--no-open"], { cwd: repo });
    expect(piped.exitCode).toBe(0);
    expect(piped.stdout.trim().startsWith("{")).toBe(true);
    expect(piped.json.diff_id).toMatch(hex64);

    // A terminal: human text naming the review, diff and URL.
    const tty = runOnTty(["show", "HEAD", "--no-open", "--repo", repo]);
    expect(tty.exitCode).toBe(0);
    expect(tty.output).not.toContain("{");
    expect(tty.output).toContain(piped.json.review_id);
    expect(tty.output).toContain(piped.json.diff_id.slice(0, 12));
    expect(tty.output).toContain(`polygloss://diff/${piped.json.diff_id}`);

    // --json wins on a terminal too.
    const forced = runOnTty([
      "show",
      "HEAD",
      "--no-open",
      "--json",
      "--repo",
      repo,
    ]);
    expect(forced.exitCode).toBe(0);
    const text = forced.output;
    const json = JSON.parse(
      text.slice(text.indexOf("{"), text.lastIndexOf("}") + 1),
    );
    expect(json.diff_id).toBe(piped.json.diff_id);
  });

  test("errors exit 1 with code and message", () => {
    const repo = makeRepo("errors");
    const badRev = run(["show", "no-such-rev", "--no-open"], { cwd: repo });
    expect(badRev.exitCode).toBe(1);
    expect(badRev.json).toEqual({
      error: {
        code: "not_found",
        message: expect.stringContaining("no-such-rev"),
      },
    });

    const notRepo = join(sandbox.home, "not-a-repo");
    mkdirSync(notRepo, { recursive: true });
    const outside = run(["--no-open", notRepo]);
    expect(outside.exitCode).toBe(1);
    expect(outside.json.error.code).toBe("repo_not_found");
    expect(outside.json.error.message.length).toBeGreaterThan(0);

    // Human mode: the message on stderr (the pty merges it into the output).
    const human = runOnTty([
      "show",
      "no-such-rev",
      "--no-open",
      "--repo",
      repo,
    ]);
    expect(human.exitCode).toBe(1);
    expect(human.output).toContain("polygloss: ");
    expect(human.output).toContain("no-such-rev");
  });

  test("linked worktree keys live review by worktree path", () => {
    const repo = makeRepo("linked-main");
    const wt = join(sandbox.home, "linked-wt");
    git(repo, ["worktree", "add", "-q", "-b", "feature", wt]);
    const wtReal = realpathSync(wt);
    writeFileSync(join(wtReal, "a.txt"), "one\nwt\nthree\n");
    writeFileSync(join(repo, "a.txt"), "one\nmain\nthree\n");

    const inWt = run(["--no-open"], { cwd: join(wtReal) });
    expect(inWt.exitCode).toBe(0);
    expect(inWt.json.review_key).toBe(
      `worktree:${wtReal}@feature#since=merge-base`,
    );
    const inMain = run(["--no-open", repo]);
    expect(inMain.exitCode).toBe(0);
    expect(inMain.json.review_key).toBe(
      `worktree:${repo}@main#since=merge-base`,
    );
    expect(inMain.json.review_id).not.toBe(inWt.json.review_id);

    // One repo row for both worktrees.
    const repos = query<{ n: number }>(
      "SELECT COUNT(DISTINCT repo_id) AS n FROM reviews WHERE id IN (?, ?)",
      inWt.json.review_id,
      inMain.json.review_id,
    );
    expect(repos.n).toBe(1);
  });

  test("snapshot pins the live state", () => {
    const repo = makeRepo("snapshot");
    writeFileSync(join(repo, "a.txt"), "one\nsnap\nthree\n");
    writeFileSync(join(repo, "new.txt"), "untracked\n");

    const snap = run(["snapshot", repo]);
    expect(snap.exitCode).toBe(0);
    expect(snap.json).toMatchObject({
      kind: "live",
      review_key: `worktree:${repo}@main#since=merge-base`,
      iteration: 1,
      url: `polygloss://diff/${snap.json.diff_id}`,
      files: 2,
      app: "skipped",
    });
    const it = query<{ pinned_by: string; snapshot_ref: string }>(
      "SELECT pinned_by, snapshot_ref FROM iterations WHERE review_id = ? AND seq = 1",
      snap.json.review_id,
    );
    expect(it.pinned_by).toBe("manual");
    expect(it.snapshot_ref).toMatch(/^refs\/polygloss\/snapshots\//);
    // The user's HEAD and branches are untouched.
    expect(git(repo, ["status", "--porcelain"])).toContain("new.txt");

    // The live review now shows the pinned iteration.
    const live = run(["--no-open", repo]);
    expect(live.json.review_id).toBe(snap.json.review_id);
    expect(live.json.iteration).toBe(1);
    expect(live.json.diff_id).toBe(snap.json.diff_id);

    // A further edit and snapshot records iteration 2; the cwd works too.
    writeFileSync(join(repo, "a.txt"), "one\nsnap again\nthree\n");
    const second = run(["snapshot"], { cwd: repo });
    expect(second.exitCode).toBe(0);
    expect(second.json.iteration).toBe(2);
    expect(second.json.review_id).toBe(snap.json.review_id);
  });

  test("the command tree has every section 14 command", () => {
    const help = run(["--help"]);
    expect(help.exitCode).toBe(0);
    for (const name of [
      "show",
      "compare",
      "open",
      "snapshot",
      "mcp",
      "wait",
      "reviews",
      "threads",
      "thread",
      "reply",
      "resolve",
      "unresolve",
      "edit",
      "delete",
      "comment",
      "wait-review",
      "rereview",
      "focus",
    ])
      expect(help.stdout).toMatch(new RegExp(`\\n  ${name} `));
    for (const flag of [
      "--repo",
      "--json",
      "--no-open",
      "--agent",
      "--session",
      "--since",
    ])
      expect(help.stdout).toContain(flag);
    // Global flags work after a subcommand, and `wait` takes --session.
    expect(run(["wait", "--help"]).stdout).toContain("--session");
    expect(run(["compare", "--help"]).stdout).toContain("--direct");
  });
});

/** A short data dir, so the socket path fits in `sun_path` (104 bytes). */
function shortDataDir(): string {
  const dir = mkdtempSync("/tmp/pgd-");
  afterAll(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

/** A fake app: answers `hello` and `open` on `<dataDir>/polygloss.sock`
 * and records every request. */
function fakeAppSource(): string {
  return `
import { appendFileSync } from "node:fs";
const dir = process.env.POLYGLOSS_DATA_DIR;
appendFileSync(dir + "/launched.log", JSON.stringify(process.argv.slice(2)) + "\\n");
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
          : { status: "opened", review_id: req.review_id ?? null, diff_id: req.diff_id ?? null };
        socket.write(JSON.stringify({ id: req.id, ok: true, result }) + "\\n");
      }
    },
  },
});
setTimeout(() => process.exit(0), Number(process.env.FAKE_APP_LIFETIME_MS ?? 20000));
`;
}

function readOps(dataDir: string): Json[] {
  const file = join(dataDir, "ops.log");
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf8")
    .trim()
    .split("\n")
    .filter((l) => l !== "")
    .map((l) => JSON.parse(l));
}

describe("human commands show the review in the app", () => {
  test("a running app is asked to open the review with activate", async () => {
    const dataDir = shortDataDir();
    const env = { ...sandbox.env, POLYGLOSS_DATA_DIR: dataDir };
    const script = join(dataDir, "fake-app.ts");
    writeFileSync(script, fakeAppSource());
    const app = Bun.spawn([process.execPath, script], {
      env: { ...env, FAKE_APP_LIFETIME_MS: "30000" },
      stdout: "ignore",
      stderr: "inherit",
    });
    try {
      const sock = join(dataDir, "polygloss.sock");
      for (let i = 0; i < 200 && !existsSync(sock); i++) await Bun.sleep(25);
      expect(existsSync(sock)).toBe(true);

      const repo = makeRepo("app-running");
      commitFile(repo, "a.txt", "app\n", "second");
      const r = run(["show", "HEAD"], { env, cwd: repo });
      expect(r.stderr).toBe("");
      expect(r.exitCode).toBe(0);
      expect(r.json.app).toBe("opened");
      const opens = readOps(dataDir).filter((op) => op.op === "open");
      expect(opens).toEqual([
        {
          v: 1,
          id: expect.any(Number),
          op: "open",
          review_id: r.json.review_id,
          activate: true,
        },
      ]);

      // `open <prefix>` names the diff, so the app shows that diff's review.
      const byId = run(["open", r.json.diff_id.slice(0, 10)], { env });
      expect(byId.exitCode).toBe(0);
      expect(byId.json.app).toBe("opened");
      const last = readOps(dataDir)
        .filter((op) => op.op === "open")
        .at(-1);
      expect(last).toMatchObject({ diff_id: r.json.diff_id, activate: true });
    } finally {
      app.kill();
      await app.exited;
    }
  });

  test("the app is launched through the test override when it is not running", async () => {
    const dataDir = shortDataDir();
    const appBin = join(dataDir, "fake-polygloss");
    writeFileSync(appBin, `#!${process.execPath}\n${fakeAppSource()}`);
    chmodSync(appBin, 0o755);
    const env = {
      ...sandbox.env,
      POLYGLOSS_DATA_DIR: dataDir,
      POLYGLOSS_TEST: "1",
      POLYGLOSS_APP_BIN: appBin,
      FAKE_APP_LIFETIME_MS: "5000",
    };
    const repo = makeRepo("app-launch");
    writeFileSync(join(repo, "a.txt"), "launch\n");
    const r = run([repo], { env });
    expect(r.stderr).toBe("");
    expect(r.exitCode).toBe(0);
    expect(r.json.app).toBe("launched");
    expect(existsSync(join(dataDir, "launched.log"))).toBe(true);
    expect(readOps(dataDir).filter((op) => op.op === "open")).toEqual([
      {
        v: 1,
        id: expect.any(Number),
        op: "open",
        review_id: r.json.review_id,
        activate: true,
      },
    ]);
  });

  test("an app that cannot start is an app_unavailable error", () => {
    const dataDir = shortDataDir();
    // Test mode without POLYGLOSS_APP_BIN refuses to launch anything.
    const env = {
      ...sandbox.env,
      POLYGLOSS_DATA_DIR: dataDir,
      POLYGLOSS_TEST: "1",
    };
    const repo = makeRepo("app-unavailable");
    commitFile(repo, "a.txt", "x\n", "second");
    const r = run(["show", "HEAD"], { env, cwd: repo });
    expect(r.exitCode).toBe(1);
    expect(r.json.error.code).toBe("app_unavailable");
    // The review was recorded; the message says how to reach it.
    expect(r.json.error.message).toContain("polygloss://diff/");
  });
});
