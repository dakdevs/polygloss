// Hidden `polygloss-cli debug seed|human-comment|human-viewed|human-submit`
// (T4.4, OQ-P4): the human's side of a review for bun tests. Test-only: they
// refuse to run without POLYGLOSS_TEST=1.
import { Database } from "bun:sqlite";
import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());
const testEnv = { ...sandbox.env, POLYGLOSS_TEST: "1" };

function git(repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env: sandbox.env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
  return r.stdout.toString();
}

/** A repo with one commit and an uncommitted edit of `a.txt`. */
function liveRepo(name: string): string {
  const repo = join(sandbox.home, name);
  mkdirSync(repo, { recursive: true });
  git(repo, ["init", "-q", "-b", "main"]);
  writeFileSync(join(repo, "a.txt"), "one\ntwo\nthree\n");
  git(repo, ["add", "a.txt"]);
  git(repo, ["commit", "-q", "-m", "init"]);
  writeFileSync(join(repo, "a.txt"), "one\nTWO\nthree\nfour\n");
  return repo;
}

function debug(
  args: string[],
  env: Record<string, string> = testEnv,
): { json: Record<string, unknown>; stderr: string; exitCode: number } {
  const r = Bun.spawnSync([cliBin(), "debug", ...args], { env });
  const stdout = r.stdout.toString();
  return {
    json: stdout.trim() === "" ? {} : JSON.parse(stdout),
    stderr: r.stderr.toString(),
    exitCode: r.exitCode,
  };
}

/** The sandbox store. Opened read-write: a read-only open of a WAL database
 * fails once the last writer has exited and removed its `-shm` file. */
function db(): Database {
  return new Database(join(sandbox.dataDir, "polygloss.db"), {
    readwrite: true,
  });
}

describe("polygloss-cli debug human commands", () => {
  test("refuse to run without POLYGLOSS_TEST=1", () => {
    const repo = liveRepo("no-test-env");
    for (const args of [
      ["seed", "--repo", repo],
      ["human-comment", "--review", "r", "--body", "x"],
      ["human-viewed", "--review", "r", "--path", "a.txt"],
      ["human-submit", "--review", "r"],
    ]) {
      const r = debug(args, sandbox.env);
      expect(r.exitCode).toBe(1);
      expect(r.stderr).toContain("test-only");
      expect(r.stderr).toContain("POLYGLOSS_TEST=1");
    }
  });

  test("seed, comment, viewed and submit act as the human", () => {
    const repo = liveRepo("human-loop");
    const seeded = debug(["seed", "--repo", repo, "--since", "HEAD"]);
    expect(seeded.exitCode).toBe(0);
    expect(seeded.json.iteration).toBe(1);
    expect(seeded.json.files).toBe(1);
    expect(seeded.json.diff_id).toMatch(/^[0-9a-f]{64}$/);
    const reviewId = seeded.json.review_id as string;
    expect(reviewId).toMatch(/^[0-9a-f-]{36}$/);

    const comment = debug([
      "human-comment",
      "--review",
      reviewId,
      "--path",
      "a.txt",
      "--line",
      "2",
      "--body",
      "Why uppercase?",
    ]);
    expect(comment.exitCode).toBe(0);
    const threadId = comment.json.thread_id as string;
    expect(comment.json.diff_id).toBe(seeded.json.diff_id);
    const reply = debug([
      "human-comment",
      "--review",
      reviewId,
      "--reply-to",
      threadId,
      "--body",
      "And line 4?",
    ]);
    expect(reply.exitCode).toBe(0);
    expect(reply.json.thread_id).toBe(threadId);

    let conn = db();
    try {
      const thread = conn
        .query(
          "SELECT subject, path, side, start_line, line, created_by_kind FROM threads WHERE id = ?",
        )
        .get(threadId);
      expect(thread).toEqual({
        subject: "line",
        path: "a.txt",
        side: "new",
        start_line: 2,
        line: 2,
        created_by_kind: "human",
      });
      // Both comments are drafts until the human submits.
      const drafts = conn
        .query(
          "SELECT COUNT(*) AS n FROM comments WHERE thread_id = ? AND published_at IS NULL AND author_kind = 'human'",
        )
        .get(threadId) as { n: number };
      expect(drafts.n).toBe(2);
    } finally {
      conn.close();
    }

    const viewed = debug([
      "human-viewed",
      "--review",
      reviewId,
      "--path",
      "a.txt",
    ]);
    expect(viewed.exitCode).toBe(0);
    expect(viewed.json).toEqual({ path: "a.txt", viewed: true });

    const submitted = debug([
      "human-submit",
      "--review",
      reviewId,
      "--verdict",
      "request-changes",
      "--summary",
      "Please explain.",
    ]);
    expect(submitted.exitCode).toBe(0);
    expect(submitted.json).toMatchObject({
      review_id: reviewId,
      verdict: "request_changes",
      iteration: 1,
      comment_count: 2,
    });
    expect(typeof submitted.json.seq).toBe("number");

    conn = db();
    try {
      const published = conn
        .query(
          "SELECT COUNT(*) AS n FROM comments WHERE thread_id = ? AND published_at IS NOT NULL",
        )
        .get(threadId) as { n: number };
      expect(published.n).toBe(2);
      const review = conn
        .query("SELECT status FROM reviews WHERE id = ?")
        .get(reviewId) as { status: string };
      expect(review.status).toBe("changes_requested");
      const viewedRows = conn
        .query("SELECT COUNT(*) AS n FROM viewed_files WHERE path = 'a.txt'")
        .get() as { n: number };
      expect(viewedRows.n).toBe(1);
    } finally {
      conn.close();
    }

    const unviewed = debug([
      "human-viewed",
      "--review",
      reviewId,
      "--path",
      "a.txt",
      "--unset",
    ]);
    expect(unviewed.json).toEqual({ path: "a.txt", viewed: false });
  });

  test("seed a commit review and reject an unknown review", () => {
    const repo = liveRepo("commit-review");
    git(repo, ["commit", "-q", "-am", "second"]);
    const seeded = debug(["seed", "--repo", repo, "--commit", "HEAD"]);
    expect(seeded.exitCode).toBe(0);
    expect(seeded.json.files).toBe(1);

    const missing = debug([
      "human-submit",
      "--review",
      "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
    ]);
    expect(missing.exitCode).toBe(1);
    expect(missing.stderr.length).toBeGreaterThan(0);
  });
});
