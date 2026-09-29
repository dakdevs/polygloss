import { afterAll, describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, realpathSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  buildParityRepo,
  makeParityRepo,
} from "../../scripts/make-parity-repo";
import { cliBin } from "../support/bins";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

// The scripts under test use this checkout's already-built CLI (bun preload).
const env = { ...sandbox.env, POLYGLOSS_CLI_BIN: cliBin() };

let counter = 0;
function scratch(): string {
  return join(sandbox.home, `parity-${counter++}`);
}

function run(argv: string[]): {
  code: number | null;
  stdout: string;
  stderr: string;
} {
  const r = Bun.spawnSync(argv, { cwd: repoRoot, env });
  return {
    code: r.exitCode,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

function gitParity(args: string[]): ReturnType<typeof run> {
  return run(["bun", "scripts/git-parity.ts", ...args]);
}

function debugParity(args: string[]): {
  files: number;
  identical: number;
  mismatches: Array<{ path: string; ours: string; git: string }>;
} {
  const r = run([cliBin(), "debug", "parity", ...args, "--json"]);
  expect(r.stderr).toBe("");
  expect(r.code).toBe(0);
  return JSON.parse(r.stdout);
}

/** fixtures/parity/<case>/{old,new} as one repo: `<case>.txt` old → new. */
function fixturesRepo(): { path: string; cases: number } {
  const dir = join(repoRoot, "fixtures", "parity");
  const cases = readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => e.name)
    .sort();
  expect(cases.length).toBeGreaterThanOrEqual(9);
  const side = (name: "old" | "new") =>
    cases.map(
      (c) =>
        [`cases/${c}.txt`, readFileSync(join(dir, c, name))] as [
          string,
          Buffer,
        ],
    );
  const repo = buildParityRepo({
    out: scratch(),
    base: side("old"),
    head: side("new"),
  });
  return { path: repo.path, cases: cases.length };
}

// The first example of Myers' paper: git's Myers and Histogram disagree on it.
const myersPaperOld = "a\nb\nc\na\nb\nb\na\n";
const myersPaperNew = "c\nb\na\nb\na\nc\n";

describe("polygloss-cli debug parity", () => {
  test("debug subcommand is hidden from help", () => {
    const help = run([cliBin(), "--help"]);
    expect(help.code).toBe(0);
    expect(help.stdout).not.toMatch(/debug/i);
    const sub = run([cliBin(), "debug", "parity", "--help"]);
    expect(sub.code).toBe(0);
    for (const flag of ["--repo", "--base", "--head", "--algorithm", "--json"])
      expect(sub.stdout).toContain(flag);
  });

  test("compares every text M/R pair of the parity fixtures", () => {
    const repo = fixturesRepo();
    const report = debugParity([
      "--repo",
      repo.path,
      "--base",
      "parity-base",
      "--head",
      "parity-head",
    ]);
    expect(report.mismatches).toEqual([]);
    expect(report.files).toBe(repo.cases);
    expect(report.identical).toBe(report.files);
  });

  test("reports ours and git's text for a mismatch", () => {
    const repo = buildParityRepo({
      out: scratch(),
      base: [["paper.txt", myersPaperOld]],
      head: [["paper.txt", myersPaperNew]],
    }).path;
    const args = [
      "--repo",
      repo,
      "--base",
      "parity-base",
      "--head",
      "parity-head",
    ];
    expect(debugParity(args)).toEqual({
      files: 1,
      identical: 1,
      mismatches: [],
    });
    const report = debugParity([...args, "--algorithm", "histogram"]);
    expect(report.files).toBe(1);
    expect(report.identical).toBe(0);
    expect(report.mismatches.map((m) => m.path)).toEqual(["paper.txt"]);
    const [m] = report.mismatches;
    expect(m?.git.startsWith("@@ -1,7 +1,6 @@\n")).toBe(true);
    expect(m?.ours).not.toBe(m?.git);
  });

  test("skips binary, symlink, added, deleted and mode-only changes", () => {
    const repo = buildParityRepo({
      out: scratch(),
      base: [
        ["text.txt", "one\ntwo\n"],
        ["blob.bin", Buffer.from([1, 0, 2, 3])],
        ["gone.txt", "bye\n"],
        ["same.txt", "same\n"],
      ],
      head: [
        ["text.txt", "one\n2\n"],
        ["blob.bin", Buffer.from([1, 0, 2, 4])],
        ["new.txt", "hi\n"],
        ["same.txt", "same\n", "100755"],
      ],
    }).path;
    const report = debugParity([
      "--repo",
      repo,
      "--base",
      "parity-base",
      "--head",
      "parity-head",
    ]);
    expect(report).toEqual({ files: 1, identical: 1, mismatches: [] });
  });

  test("fails on a revision that does not exist", () => {
    const repo = buildParityRepo({
      out: scratch(),
      base: [["a.txt", "a\n"]],
      head: [["a.txt", "b\n"]],
    }).path;
    const r = run([
      cliBin(),
      "debug",
      "parity",
      "--repo",
      repo,
      "--base",
      "no-such-rev",
      "--head",
      "parity-head",
      "--json",
    ]);
    expect(r.code).toBe(1);
    expect(r.stdout).toBe("");
    expect(r.stderr).toContain("no-such-rev");
  });
});

describe("scripts/git-parity.ts", () => {
  test("git-parity reports 100% on the parity fixtures repo", () => {
    const repo = fixturesRepo();
    const r = gitParity([
      "--repo",
      repo.path,
      "--range",
      "parity-base..parity-head",
      "--min-rate",
      "1",
    ]);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
    expect(r.stdout).toMatch(new RegExp(`identical\\s+${repo.cases}\n`));
    expect(r.stdout).toMatch(/rate\s+100\.000%/);
  });

  test("git-parity fails below min rate", () => {
    const repo = buildParityRepo({
      out: scratch(),
      base: [
        ["paper.txt", myersPaperOld],
        ["ok.txt", "keep\nold\n"],
      ],
      head: [
        ["paper.txt", myersPaperNew],
        ["ok.txt", "keep\nnew\n"],
      ],
    }).path;
    const args = ["--repo", repo, "--range", "parity-base..parity-head"];
    expect(gitParity(args).code).toBe(0);
    const mismatchDir = join(scratch(), "mismatches");
    const r = gitParity([
      ...args,
      "--algorithm",
      "histogram",
      "--min-rate",
      "0.999",
      "--mismatches-dir",
      mismatchDir,
    ]);
    expect(r.code).toBe(1);
    expect(r.stdout).toMatch(/identical\s+1\b/);
    expect(r.stdout).toMatch(/rate\s+50\.000%/);
    expect(r.stdout).toContain("paper.txt");
    expect(r.stderr).toContain("below the minimum");
    const files = readdirSync(mismatchDir).sort();
    expect(files).toEqual(["0001.git.diff", "0001.ours.diff", "summary.json"]);
    const summary = JSON.parse(
      readFileSync(join(mismatchDir, "summary.json"), "utf8"),
    );
    expect(summary.mismatches).toEqual([
      { path: "paper.txt", ours: "0001.ours.diff", git: "0001.git.diff" },
    ]);
  });

  test("--corpus takes the repo and range from the corpora manifest", () => {
    const root = join(scratch(), "corpora");
    const made = Bun.spawnSync(["bun", "benches/corpora/make-typical.ts"], {
      cwd: repoRoot,
      env: { ...env, POLYGLOSS_CORPORA: root },
    });
    expect(made.stderr.toString()).toBe("");
    expect(made.exitCode).toBe(0);
    const r = Bun.spawnSync(
      [
        "bun",
        "scripts/git-parity.ts",
        "--corpus",
        "typical",
        "--min-rate",
        "0",
      ],
      { cwd: repoRoot, env: { ...env, POLYGLOSS_CORPORA: root } },
    );
    expect(r.stderr.toString()).toBe("");
    expect(r.exitCode).toBe(0);
    const out = r.stdout.toString();
    expect(out).toContain(
      `repo        ${join(root, "generated", "typical")}\n`,
    );
    expect(out).toContain("range       corpus-base..corpus-head\n");
    expect(out).toMatch(/files\s+[1-9]\d*\n/);
  });

  test("--corpus cannot be combined with --repo or --range", () => {
    for (const extra of [
      ["--repo", sandbox.home],
      ["--range", "a..b"],
    ]) {
      const r = gitParity(["--corpus", "typical", ...extra]);
      expect(r.code).toBe(2);
      expect(r.stderr).toContain("--corpus");
    }
    expect(gitParity(["--corpus", "nope"]).code).toBe(2);
  });

  test("--corpus names the command that creates a missing corpus", () => {
    const r = Bun.spawnSync(
      ["bun", "scripts/git-parity.ts", "--corpus", "huge-file"],
      {
        cwd: repoRoot,
        env: { ...env, POLYGLOSS_CORPORA: join(scratch(), "empty") },
      },
    );
    expect(r.exitCode).toBe(2);
    expect(r.stderr.toString()).toContain(
      "bun benches/corpora/make-huge-file.ts",
    );
  });

  test("rejects a range without two dots", () => {
    const r = gitParity(["--repo", sandbox.home, "--range", "main"]);
    expect(r.code).toBe(2);
    expect(r.stderr).toContain("--range");
  });
});

describe("scripts/make-parity-repo.ts", () => {
  function trees(path: string): string[] {
    const r = Bun.spawnSync(
      [
        "git",
        "-C",
        path,
        "rev-parse",
        "parity-base^{tree}",
        "parity-head^{tree}",
        "parity-head",
      ],
      { env: sandbox.env },
    );
    expect(r.exitCode).toBe(0);
    return r.stdout.toString().trim().split("\n");
  }

  test("parity repo is deterministic for a seed", () => {
    const a = makeParityRepo({ seed: 7, files: 60, out: scratch() });
    const b = makeParityRepo({ seed: 7, files: 60, out: scratch() });
    const c = makeParityRepo({ seed: 8, files: 60, out: scratch() });
    expect(trees(a.path)).toEqual(trees(b.path));
    expect(trees(c.path)[1]).not.toBe(trees(a.path)[1]);
  });

  test("CLI prints the repo path and refuses a non-empty --out", () => {
    const out = scratch();
    const first = run([
      "bun",
      "scripts/make-parity-repo.ts",
      "--seed",
      "3",
      "--files",
      "20",
      "--out",
      out,
    ]);
    expect(first.stderr).toBe("");
    expect(first.code).toBe(0);
    expect(first.stdout).toBe(`${realpathSync(out)}\n`);
    const again = run([
      "bun",
      "scripts/make-parity-repo.ts",
      "--seed",
      "3",
      "--files",
      "20",
      "--out",
      out,
    ]);
    expect(again.code).toBe(2);
    expect(again.stderr).toContain("not empty");
  });

  test("covers every language and every edit kind", () => {
    const repo = makeParityRepo({ seed: 1, files: 200, out: scratch() });
    const exts = new Set(repo.files.map((f) => f.split(".").pop()));
    expect([...exts].sort()).toEqual(["go", "md", "py", "rs", "ts"]);
    for (const kind of [
      "insert",
      "delete",
      "move-block",
      "re-indent",
      "whitespace-only",
      "crlf",
      "drop-trailing-newline",
      "rename",
    ])
      expect({ kind, used: repo.edits[kind] ?? 0 }).not.toEqual({
        kind,
        used: 0,
      });
    const status = Bun.spawnSync(
      ["git", "-C", repo.path, "status", "--porcelain"],
      { env: sandbox.env },
    );
    expect(status.stdout.toString()).toBe("");
  });

  test(
    "seed 1 with 500 files reaches the 99.9% parity target",
    () => {
      const repo = makeParityRepo({ seed: 1, files: 500, out: scratch() });
      const r = gitParity([
        "--repo",
        repo.path,
        "--range",
        "parity-base..parity-head",
        "--min-rate",
        "0.999",
      ]);
      expect(r.stderr).toBe("");
      expect(r.code).toBe(0);
    },
    { timeout: 120_000 },
  );
});
