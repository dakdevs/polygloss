// scripts/release-notes.ts (ADR-0019) on a fixture repo in a sandbox: the
// expected Markdown is written out by hand from the fixture below, with the
// commit ids git gave each fixture commit.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { renderNotes } from "../../scripts/release-notes";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts", "release-notes.ts");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

const repo = join(sandbox.home, "fixture");
const sha: Record<string, string> = {};

function git(args: string[]): string {
  const r = Bun.spawnSync(["git", ...args], { cwd: repo, env: sandbox.env });
  if (r.exitCode !== 0)
    throw new Error(`git ${args.join(" ")}: ${r.stderr.toString()}`);
  return r.stdout.toString().trim();
}

/** Writes `files` and commits them as `subject`; `sha[key]` is the commit. */
function commit(key: string, subject: string, files: Record<string, string>) {
  for (const [path, text] of Object.entries(files))
    writeFileSync(join(repo, path), text);
  git(["add", "-A"]);
  git(["commit", "-q", "-m", subject]);
  sha[key] = git(["rev-parse", "HEAD"]);
}

beforeAll(() => {
  mkdirSync(repo);
  git(["init", "-q", "-b", "main"]);
  commit("c1", "feat(app): open a review window", { "app.txt": "a\nb\nc\n" });
  git(["tag", "v20261005.1"]);
  commit("c2", "fix(cli): exit 2 on a bad <rev>", { "cli.txt": "1\n2\n" });
  commit("c3", "docs: describe `polygloss <rev>` and <base>", {
    "docs.md": "d\n",
  });
  commit("c4", "ci: cache the shared build dir", { "ci.yml": "x\n" });
  commit("c5", "wip(release): checkpoint", { "wip.txt": "w\n" });
  git(["checkout", "-q", "-b", "side"]);
  commit("c6", "perf(diff): skip unchanged hunks", { "app.txt": "a\nB\nc\n" });
  git(["checkout", "-q", "main"]);
  commit("c7", "refactor(core): split the store", { "core.txt": "1\n2\n" });
  git(["merge", "-q", "--no-ff", "-m", "merge(M6): side", "side"]);
  commit("c8", "Update the README", { README: "r\n" });
  commit("c9", "fix(cli): exit 2 on a bad <rev>", { "cli.txt": "1\n" });
  commit("c10", "feat!: drop the old socket", { "socket.txt": "s\n" });
  commit("c11", "test(app): cover the palette", { "app.test": "t\n" });
  commit("c12", "fixup! test(app): cover the palette", {
    "app.test": "t\nu\n",
  });
  commit("c13", "chore: bump bun", { "deps.txt": "b\n" });
});

/** `[short](commit link)` for fixture commit `key`. */
const link = (key: string) =>
  `[${sha[key]!.slice(0, 7)}](https://github.com/dakdevs/polygloss/commit/${sha[key]})`;

function notes(args: string[]): {
  code: number;
  stdout: string;
  stderr: string;
} {
  const r = Bun.spawnSync(["bun", script, ...args], {
    cwd: repo,
    env: sandbox.env,
  });
  return {
    code: r.exitCode ?? -1,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

describe("bun scripts/release-notes.ts", () => {
  test("groups the commits since the previous tag and links the full diff", () => {
    const r = notes(["--version", "20261005.2", "--previous", "v20261005.1"]);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
    // From c1 to c13: 12 commits besides the merge; ten files differ
    // (app.txt b->B, cli.txt +1, one line each in docs.md, ci.yml, wip.txt,
    // README, socket.txt and deps.txt, two in core.txt and app.test).
    expect(r.stdout).toBe(
      `12 commits · 10 files changed · +12 −1 since v20261005.1

### Features

- drop the old socket (${link("c10")})

### Fixes

- **cli:** exit 2 on a bad &lt;rev> (${link("c9")}, ${link("c2")})

### Performance

- **diff:** skip unchanged hunks (${link("c6")})

### Changes

- Update the README (${link("c8")})
- **core:** split the store (${link("c7")})

### Documentation

- describe \`polygloss <rev>\` and &lt;base> (${link("c3")})

<details>
<summary>Maintenance (3)</summary>

- bump bun (${link("c13")})
- **app:** cover the palette (${link("c11")})
- cache the shared build dir (${link("c4")})

</details>

**Full diff:** [v20261005.1...v20261005.2](https://github.com/dakdevs/polygloss/compare/v20261005.1...v20261005.2)
`,
    );
  });

  test("the first release covers the whole history and links its commit list", () => {
    const r = notes(["--version", "20261005.1", "--repo", "someone/fork"]);
    expect(r.code).toBe(0);
    const at = (key: string) =>
      link(key).replace("dakdevs/polygloss", "someone/fork");
    // Every file at c13, from nothing: 3 + 1 + 1 + 1 + 1 + 2 + 1 + 1 + 2 + 1 lines.
    expect(r.stdout).toBe(
      `First release · 13 commits · 10 files changed · +14 −0

### Features

- drop the old socket (${at("c10")})
- **app:** open a review window (${at("c1")})

### Fixes

- **cli:** exit 2 on a bad &lt;rev> (${at("c9")}, ${at("c2")})

### Performance

- **diff:** skip unchanged hunks (${at("c6")})

### Changes

- Update the README (${at("c8")})
- **core:** split the store (${at("c7")})

### Documentation

- describe \`polygloss <rev>\` and &lt;base> (${at("c3")})

<details>
<summary>Maintenance (3)</summary>

- bump bun (${at("c13")})
- **app:** cover the palette (${at("c11")})
- cache the shared build dir (${at("c4")})

</details>

**All commits:** [v20261005.1](https://github.com/someone/fork/commits/v20261005.1)
`,
    );
    // An empty --previous (release.yml before the first tag) is none.
    expect(
      notes([
        "--version",
        "20261005.1",
        "--previous",
        "",
        "--repo",
        "someone/fork",
      ]).stdout,
    ).toBe(r.stdout);
  });

  test("--sparkle unfolds Maintenance, whose <details> Sparkle would print as text", () => {
    const r = notes([
      "--version",
      "20261005.2",
      "--previous",
      "v20261005.1",
      "--sparkle",
    ]);
    expect(r.code).toBe(0);
    expect(r.stdout).not.toContain("<details>");
    expect(r.stdout).not.toContain("<summary>");
    expect(r.stdout).toContain(
      `### Documentation

- describe \`polygloss <rev>\` and &lt;base> (${link("c3")})

### Maintenance

- bump bun (${link("c13")})
- **app:** cover the palette (${link("c11")})
- cache the shared build dir (${link("c4")})

**Full diff:**`,
    );
  });

  test("--commit picks the release commit and --out writes the file", () => {
    const out = join(sandbox.home, "notes.md");
    const r = notes([
      "--version",
      "20261005.2",
      "--previous",
      "v20261005.1",
      "--commit",
      sha.c3!,
      "--out",
      out,
    ]);
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("");
    expect(readFileSync(out, "utf8")).toBe(
      `2 commits · 2 files changed · +3 −0 since v20261005.1

### Fixes

- **cli:** exit 2 on a bad &lt;rev> (${link("c2")})

### Documentation

- describe \`polygloss <rev>\` and &lt;base> (${link("c3")})

**Full diff:** [v20261005.1...v20261005.2](https://github.com/dakdevs/polygloss/compare/v20261005.1...v20261005.2)
`,
    );
  });

  test("a release of only merges and work in progress says so", () => {
    const r = notes([
      "--version",
      "20261005.2",
      "--previous",
      sha.c4!,
      "--commit",
      sha.c5!,
    ]);
    expect(r.code).toBe(0);
    expect(r.stdout).toBe(
      `1 commit · 1 file changed · +1 −0 since ${sha.c4}

No changes besides merges and work in progress.

**Full diff:** [${sha.c4}...v20261005.2](https://github.com/dakdevs/polygloss/compare/${sha.c4}...v20261005.2)
`,
    );
  });

  test("an unknown previous tag fails with git's message and writes nothing", () => {
    const r = notes(["--version", "20261005.2", "--previous", "v20261004.9"]);
    expect(r.code).toBe(1);
    expect(r.stdout).toBe("");
    expect(r.stderr).toContain("v20261004.9");
  });

  test("usage errors exit 2", () => {
    for (const args of [
      [],
      ["--version", "1.2.3"],
      ["--version", "v20261005.1"],
      ["--version", "20261005.1", "--repo", "not a repo"],
      ["--version", "20261005.1", "--commit", "--help"],
      ["--version", "20261005.1", "--nope"],
    ])
      expect({ args, code: notes(args).code }).toEqual({ args, code: 2 });
  });
});

describe("release.yml's notes step", () => {
  /** Runs the step in the fixture, with the scripts it calls copied in. */
  function step(env: Record<string, string>): {
    code: number;
    github: string;
    sparkle: string;
  } {
    const wf = Bun.YAML.parse(
      readFileSync(join(repoRoot, ".github/workflows/release.yml"), "utf8"),
    ) as { jobs: { release: { steps: { name?: string; run?: string }[] } } };
    const run = wf.jobs.release.steps.find(
      (s) => s.name === "Write the release notes",
    )?.run;
    expect(run).toBeDefined();
    for (const f of ["release-notes.ts", "release-version.ts"])
      cpSync(join(repoRoot, "scripts", f), join(repo, "scripts", f));
    const r = Bun.spawnSync(["bash", "-e", "-c", run!], {
      cwd: repo,
      env: {
        ...sandbox.env,
        POLYGLOSS_VERSION: "20261005.2",
        GITHUB_REPOSITORY: "someone/fork",
        ...env,
      },
    });
    const read = (flavor: string) =>
      readFileSync(join(repo, "dist", `release-notes-${flavor}.md`), "utf8");
    return {
      code: r.exitCode ?? -1,
      github: read("github"),
      sparkle: read("sparkle"),
    };
  }

  beforeAll(() => mkdirSync(join(repo, "scripts")));

  test("writes both flavors for the release commit since the previous tag", () => {
    const r = step({
      POLYGLOSS_PREVIOUS_TAG: "v20261005.1",
      GITHUB_SHA: sha.c13!,
    });
    expect(r.code).toBe(0);
    expect(r.github).toStartWith(
      "12 commits · 10 files changed · +12 −1 since v20261005.1\n",
    );
    expect(r.github).toContain("<details>\n<summary>Maintenance (3)</summary>");
    expect(r.github).toEndWith(
      "**Full diff:** [v20261005.1...v20261005.2](https://github.com/someone/fork/compare/v20261005.1...v20261005.2)\n",
    );
    expect(r.sparkle).not.toContain("<details>");
    expect(r.sparkle).toContain("\n### Maintenance\n");
  });

  test("an empty previous tag makes a first release's notes, for GITHUB_SHA", () => {
    const r = step({ POLYGLOSS_PREVIOUS_TAG: "", GITHUB_SHA: sha.c3! });
    expect(r.code).toBe(0);
    expect(r.github).toStartWith(
      "First release · 3 commits · 3 files changed · +6 −0\n",
    );
    expect(r.github).toEndWith(
      "**All commits:** [v20261005.2](https://github.com/someone/fork/commits/v20261005.2)\n",
    );
  });
});

describe("renderNotes", () => {
  const stat = { files: 1, insertions: 2, deletions: 3 };
  const feats = (n: number) =>
    Array.from({ length: n }, (_, i) => ({
      sha: `${String(n - i).padStart(2, "0")}${"0".repeat(38)}`,
      subject: `feat: feature ${n - i}`,
    }));

  test("a long section shows 20 entries and links the rest to the compare view", () => {
    const md = renderNotes({
      version: "20261006.1",
      previous: "v20261005.4",
      repo: "dakdevs/polygloss",
      commits: feats(23),
      stat,
      sparkle: false,
    });
    const entries = md.split("\n").filter((l) => l.startsWith("- feature"));
    expect(entries.length).toBe(20);
    expect(entries[0]).toStartWith("- feature 23 ([2300000](");
    expect(entries[19]).toStartWith("- feature 4 ([0400000](");
    expect(md).toContain(
      "\n- and 3 more ([full diff](https://github.com/dakdevs/polygloss/compare/v20261005.4...v20261006.1))\n",
    );
    expect(md).toStartWith(
      "23 commits · 1 file changed · +2 −3 since v20261005.4\n",
    );
  });

  test("a first release shows 10 entries a section and links the commit list", () => {
    const md = renderNotes({
      version: "20261006.1",
      previous: null,
      repo: "dakdevs/polygloss",
      commits: feats(13),
      stat,
      sparkle: false,
    });
    expect(md.split("\n").filter((l) => l.startsWith("- feature")).length).toBe(
      10,
    );
    expect(md).toContain(
      "\n- and 3 more ([all commits](https://github.com/dakdevs/polygloss/commits/v20261006.1))\n",
    );
  });

  test("one commit and one file are singular", () => {
    const md = renderNotes({
      version: "20261006.1",
      previous: "v20261005.4",
      repo: "dakdevs/polygloss",
      commits: feats(1),
      stat: { files: 1, insertions: 1, deletions: 0 },
      sparkle: false,
    });
    expect(md).toStartWith(
      "1 commit · 1 file changed · +1 −0 since v20261005.4\n",
    );
  });

  test("types and subjects that are no change of their own are left out", () => {
    const md = renderNotes({
      version: "20261006.1",
      previous: "v20261005.4",
      repo: "dakdevs/polygloss",
      commits: [
        "WIP: half a feature",
        "checkpoint before the restart",
        "Merge branch 'main' into side",
        "squash! feat: x",
        "amend! fix: y",
        "fix(app): checkpoint restore keeps the scroll",
        "wipe(cache): clear on start",
      ].map((subject, i) => ({ sha: `${i}`.repeat(40), subject })),
      stat,
      sparkle: false,
    });
    expect(md).toContain("- **app:** checkpoint restore keeps the scroll (");
    // An unknown type is a change, subject and all.
    expect(md).toContain("### Changes\n\n- **cache:** clear on start (");
    for (const gone of [
      "half a feature",
      "before the restart",
      "Merge",
      "x (",
      "y (",
    ])
      expect({ gone, found: md.includes(gone) }).toEqual({
        gone,
        found: false,
      });
  });
});
