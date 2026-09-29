import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, realpathSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { makeFixtureRepo } from "../support/fixture-repo";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

let counter = 0;
function scratch(): string {
  return join(sandbox.home, `fixtures-${counter++}`);
}

/** NUL-separated git output as raw byte strings (paths may not be UTF-8). */
function nulFields(bytes: Buffer): Buffer[] {
  const out: Buffer[] = [];
  let start = 0;
  for (let i = 0; i < bytes.length; i++) {
    if (bytes[i] === 0) {
      out.push(bytes.subarray(start, i));
      start = i + 1;
    }
  }
  return out;
}

function gitBytes(repo: string, args: string[]): Buffer {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env: sandbox.env });
  if (r.exitCode !== 0) throw new Error(r.stderr.toString());
  return r.stdout;
}

describe("makeFixtureRepo", () => {
  test("basic repo has two commits", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "basic" });
    expect(repo.git(["rev-list", "--count", "HEAD"]).trim()).toBe("2");
    expect(repo.git(["status", "--porcelain"])).toBe("");
    const changes = repo.git([
      "diff-tree",
      "-r",
      "--name-status",
      "HEAD~1",
      "HEAD",
    ]);
    expect(changes).toMatch(/^M\t/m);
    expect(changes).toMatch(/^A\t/m);
    expect(changes).toMatch(/^D\t/m);
  });

  test("repo path is a realpath inside dir and git runs in it", () => {
    const dir = scratch();
    const repo = makeFixtureRepo({ dir, kind: "basic" });
    expect(repo.path).toBe(realpathSync(repo.path));
    expect(repo.path.startsWith(realpathSync(dir) + "/")).toBe(true);
    expect(repo.git(["rev-parse", "--show-toplevel"]).trim()).toBe(repo.path);
  });

  test("git() throws with git's stderr on failure", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "basic" });
    expect(() => repo.git(["rev-parse", "no-such-ref"])).toThrow(/no-such-ref/);
  });

  test("fixture repos are deterministic", () => {
    const a = makeFixtureRepo({ dir: scratch(), kind: "basic" });
    const b = makeFixtureRepo({ dir: scratch(), kind: "basic" });
    expect(a.git(["rev-parse", "HEAD"])).toBe(b.git(["rev-parse", "HEAD"]));
  });

  test("the caller's global git config is never read", () => {
    const hostile = join(sandbox.home, "hostile-gitconfig");
    writeFileSync(
      hostile,
      "[commit]\n\tgpgsign = true\n[init]\n\tdefaultBranch = trunk\n[user]\n\tname = Real Person\n",
    );
    const saved = process.env.GIT_CONFIG_GLOBAL;
    process.env.GIT_CONFIG_GLOBAL = hostile;
    try {
      const repo = makeFixtureRepo({ dir: scratch(), kind: "basic" });
      expect(repo.git(["symbolic-ref", "--short", "HEAD"]).trim()).toBe("main");
      expect(repo.git(["log", "-1", "--format=%an"]).trim()).not.toBe(
        "Real Person",
      );
    } finally {
      if (saved === undefined) delete process.env.GIT_CONFIG_GLOBAL;
      else process.env.GIT_CONFIG_GLOBAL = saved;
    }
  });

  test("renames repo head commit renames files", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "renames" });
    const status = repo.git([
      "diff-tree",
      "-r",
      "-M50%",
      "--name-status",
      "HEAD~1",
      "HEAD",
    ]);
    const renames = status.split("\n").filter((l) => l.startsWith("R"));
    expect(renames.some((l) => l.startsWith("R100\t"))).toBe(true);
    expect(renames.some((l) => /^R0\d\d\t/.test(l))).toBe(true);
  });

  test("sha256 repo reports sha256 object format", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "sha256" });
    expect(repo.git(["rev-parse", "--show-object-format"]).trim()).toBe(
      "sha256",
    );
    expect(repo.git(["rev-parse", "HEAD"]).trim()).toMatch(/^[0-9a-f]{64}$/);
    expect(repo.git(["rev-list", "--count", "HEAD"]).trim()).toBe("2");
  });

  test("hostile-paths repo contains a path with a newline", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "hostile-paths" });
    const paths = nulFields(
      gitBytes(repo.path, ["ls-tree", "-r", "-z", "--name-only", "HEAD"]),
    );
    const text = paths.map((p) => p.toString("utf8"));
    expect(text.some((p) => p.includes("\n"))).toBe(true);
    expect(text.some((p) => p.includes("\t"))).toBe(true);
    expect(text.some((p) => p.includes(" "))).toBe(true);
    expect(text.some((p) => p.includes('"'))).toBe(true);
    expect(text.some((p) => /[^\x00-\x7f]/.test(p) && !p.includes("�"))).toBe(
      true,
    );
    // A path that is not valid UTF-8 (lives in the tree only; APFS rejects it).
    expect(paths.some((p) => p.includes(0xff))).toBe(true);
  });

  test("hostile-paths repo renames into and out of hostile paths", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "hostile-paths" });
    const raw = gitBytes(repo.path, [
      "diff-tree",
      "-r",
      "-z",
      "-M50%",
      "--name-status",
      "HEAD~1",
      "HEAD",
    ]);
    const fields = nulFields(raw).map((f) => f.toString("utf8"));
    const renames = fields.filter((f) => f.startsWith("R")).length;
    expect(renames).toBeGreaterThanOrEqual(2);
  });

  test("hostile-paths checkout holds every path the filesystem accepts", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "hostile-paths" });
    expect(repo.git(["status", "--porcelain"])).toBe("");
    expect(existsSync(join(repo.path, "new\nline.txt"))).toBe(true);
    expect(existsSync(join(repo.path, "moved\ninto newline.txt"))).toBe(true);
  });

  test("worktrees repo has linked worktrees sharing one common dir", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "worktrees" });
    const list = repo.git(["worktree", "list", "--porcelain"]);
    const worktrees = list
      .split("\n")
      .filter((l) => l.startsWith("worktree "))
      .map((l) => l.slice("worktree ".length));
    expect(worktrees.length).toBe(3);
    expect(list).toContain("branch refs/heads/feature");
    expect(list).toContain("detached");
    const common = (wt: string) =>
      realpathSync(
        gitBytes(wt, [
          "rev-parse",
          "--path-format=absolute",
          "--git-common-dir",
        ])
          .toString()
          .trim(),
      );
    for (const wt of worktrees) expect(common(wt)).toBe(common(repo.path));
    expect(worktrees).toContain(repo.path);
  });

  test("unborn repo has no commits but has work in progress", () => {
    const repo = makeFixtureRepo({ dir: scratch(), kind: "unborn" });
    expect(() => repo.git(["rev-parse", "--verify", "-q", "HEAD"])).toThrow();
    expect(repo.git(["symbolic-ref", "HEAD"]).trim()).toBe("refs/heads/main");
    const status = repo.git(["status", "--porcelain"]);
    expect(status).toMatch(/^A /m);
    expect(status).toMatch(/^\?\? /m);
  });

  test("make-fixture-repo.ts CLI prints the repo path", () => {
    const dir = scratch();
    const r = Bun.spawnSync(
      [
        process.execPath,
        join(repoRoot, "scripts", "make-fixture-repo.ts"),
        "sha256",
        dir,
      ],
      { env: sandbox.env },
    );
    expect(r.stderr.toString()).toBe("");
    expect(r.exitCode).toBe(0);
    const path = r.stdout.toString().trim();
    expect(existsSync(join(path, ".git"))).toBe(true);
  });

  test("make-fixture-repo.ts CLI rejects an unknown kind", () => {
    const r = Bun.spawnSync(
      [
        process.execPath,
        join(repoRoot, "scripts", "make-fixture-repo.ts"),
        "nope",
        scratch(),
      ],
      { env: sandbox.env },
    );
    expect(r.exitCode).toBe(2);
    expect(r.stderr.toString()).toContain("usage:");
  });
});
