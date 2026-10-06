// scripts/release-notes.ts (ADR-0019) on a fixture repo in a sandbox: the
// expected Markdown is written out by hand from the fixture below, with the
// commit ids git gave each fixture commit. Also release.yml's version and
// notes steps, run on clones of the fixture.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  cpSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { renderNotes } from "../../scripts/release-notes";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts", "release-notes.ts");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

const repo = join(sandbox.home, "fixture");
const sha: Record<string, string> = {};

// Each commit a minute after the last, so git's default (date) order is the
// order they were made in, which --topo-order is not.
let minute = 0;
function git(args: string[], cwd: string = repo): string {
  minute += 1;
  const date = `${1_791_226_800 + minute * 60} +0000`;
  const r = Bun.spawnSync(["git", ...args], {
    cwd,
    env: { ...sandbox.env, GIT_AUTHOR_DATE: date, GIT_COMMITTER_DATE: date },
  });
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
  // c6 on a side branch, then c7 on main, merged: newest first by date is
  // c7, c6; by topology, the merged branch first.
  git(["checkout", "-q", "-b", "side"]);
  commit("c6", "perf(diff): skip unchanged hunks", { "app.txt": "a\nB\nc\n" });
  git(["checkout", "-q", "main"]);
  commit("c7", "perf(core): T2.10.1 cache the parsed store", {
    "core.txt": "1\n2\n",
  });
  git(["merge", "-q", "--no-ff", "-m", "merge(M6): side", "side"]);
  commit("c8", "Update the README", { README: "r\n" });
  commit("c9", "fix(cli): exit 2 on a bad <rev>", { "cli.txt": "1\n" });
  commit("c10", "feat!: drop the old socket", { "socket.txt": "s\n" });
  commit("c11", "test(app): cover the palette", { "app.test": "t\n" });
  commit("c12", "fixup! test(app): cover the palette", {
    "app.test": "t\nu\n",
  });
  commit("c13", "chore: bump bun", { "deps.txt": "b\n" });
  commit("c14", "build: pin cargo-packager 0.11.8", { "build.txt": "p\n" });
  commit("c15", "style(app): wrap at 100 columns", { "style.txt": "s\n" });
  commit("c16", "Feat(viewport): S14 category sections", {
    "viewport.txt": "v\n",
  });
  commit("c17", "feat(release): release CalVer versions", {
    "release.txt": "r\n",
  });
  commit("c18", "fix(M6): integrate wave 4", { "wave.txt": "4\n" });
  commit("c19", "refactor(diff): diff hunks through one iterator", {
    "diff.txt": "i\n",
  });
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
    // From c1 to c19: 18 commits besides the merge; 16 files differ
    // (app.txt b->B, cli.txt +1, two lines in core.txt and app.test, one in
    // each of the 12 other new files).
    expect(r.stdout).toBe(
      `18 commits · 16 files changed · +18 −1 since v20261005.1

### Breaking changes

- drop the old socket (${link("c10")})

### Features

- **viewport:** category sections (${link("c16")})

### Fixes

- **cli:** exit 2 on a bad &lt;rev> (${link("c9")}, ${link("c2")})

### Performance

- **diff:** skip unchanged hunks (${link("c6")})
- **core:** cache the parsed store (${link("c7")})

### Changes

- **diff:** hunks through one iterator (${link("c19")})
- Update the README (${link("c8")})

### Documentation

- describe \`polygloss <rev>\` and &lt;base> (${link("c3")})

<details>
<summary>Maintenance (7)</summary>

- **M6:** integrate wave 4 (${link("c18")})
- **release:** CalVer versions (${link("c17")})
- **app:** wrap at 100 columns (${link("c15")})
- pin cargo-packager 0.11.8 (${link("c14")})
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
    // Every file at c19, from nothing: 3 lines in app.txt, 2 in core.txt and
    // app.test, 1 in each of the 13 others.
    expect(r.stdout).toBe(
      `First release · 19 commits · 16 files changed · +20 −0

### Breaking changes

- drop the old socket (${at("c10")})

### Features

- **viewport:** category sections (${at("c16")})
- **app:** open a review window (${at("c1")})

### Fixes

- **cli:** exit 2 on a bad &lt;rev> (${at("c9")}, ${at("c2")})

### Performance

- **diff:** skip unchanged hunks (${at("c6")})
- **core:** cache the parsed store (${at("c7")})

### Changes

- **diff:** hunks through one iterator (${at("c19")})
- Update the README (${at("c8")})

### Documentation

- describe \`polygloss <rev>\` and &lt;base> (${at("c3")})

<details>
<summary>Maintenance (7)</summary>

- **M6:** integrate wave 4 (${at("c18")})
- **release:** CalVer versions (${at("c17")})
- **app:** wrap at 100 columns (${at("c15")})
- pin cargo-packager 0.11.8 (${at("c14")})
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

- **M6:** integrate wave 4 (${link("c18")})
- **release:** CalVer versions (${link("c17")})
- **app:** wrap at 100 columns (${link("c15")})
- pin cargo-packager 0.11.8 (${link("c14")})
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

  test("a --previous that --commit does not contain is refused: its diff would run backwards", () => {
    // A newer release (c13) than the commit (c9), as when a later push
    // released first; and c6 on the side branch, not in c7's history.
    for (const [newer, older] of [
      ["c13", "c9"],
      ["c6", "c7"],
    ] as const) {
      const [previous, commit] = [sha[newer]!, sha[older]!];
      const r = notes([
        "--version",
        "20261005.3",
        "--previous",
        previous,
        "--commit",
        commit,
      ]);
      expect(r.code).toBe(2);
      expect(r.stdout).toBe("");
      expect(r.stderr).toContain(`${commit} does not contain ${previous}`);
    }
    // The tagged commit itself is no change, but not backwards.
    const same = notes([
      "--version",
      "20261005.3",
      "--previous",
      sha.c9!,
      "--commit",
      sha.c9!,
    ]);
    expect(same.code).toBe(0);
    expect(same.stdout).toStartWith(
      `0 commits · 0 files changed · +0 −0 since ${sha.c9}\n`,
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
      GITHUB_SHA: sha.c19!,
    });
    expect(r.code).toBe(0);
    expect(r.github).toStartWith(
      "18 commits · 16 files changed · +18 −1 since v20261005.1\n",
    );
    expect(r.github).toContain("<details>\n<summary>Maintenance (7)</summary>");
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

describe("release.yml's version step", () => {
  // The runner's checkout: a clone of `origin`, a bare copy of the fixture
  // whose release tags each test sets.
  const origin = join(sandbox.home, "origin.git");
  const checkout = join(sandbox.home, "checkout");

  function step(env: Record<string, string>): {
    code: number;
    output: string;
    githubEnv: Record<string, string>;
  } {
    const wf = Bun.YAML.parse(
      readFileSync(join(repoRoot, ".github/workflows/release.yml"), "utf8"),
    ) as { jobs: { release: { steps: { name?: string; run?: string }[] } } };
    const run = wf.jobs.release.steps.find(
      (s) => s.name === "Compute the version",
    )?.run;
    expect(run).toBeDefined();
    const githubEnv = join(sandbox.home, "github-env");
    writeFileSync(githubEnv, "");
    const r = Bun.spawnSync(["bash", "-e", "-c", run!], {
      cwd: checkout,
      env: { ...sandbox.env, GITHUB_ENV: githubEnv, ...env },
    });
    return {
      code: r.exitCode ?? -1,
      output: r.stdout.toString() + r.stderr.toString(),
      githubEnv: Object.fromEntries(
        readFileSync(githubEnv, "utf8")
          .split("\n")
          .filter(Boolean)
          .map((line) => [
            line.slice(0, line.indexOf("=")),
            line.slice(line.indexOf("=") + 1),
          ]),
      ),
    };
  }

  /** Release tags on origin, as `{tag: fixture commit}`; then a fresh checkout. */
  function tags(onOrigin: Record<string, string>): void {
    rmSync(origin, { recursive: true, force: true });
    rmSync(checkout, { recursive: true, force: true });
    git(["clone", "-q", "--bare", repo, origin], sandbox.home);
    git(["tag", "-d", "v20261005.1"], origin);
    for (const [tag, key] of Object.entries(onOrigin))
      git(["tag", tag, sha[key]!], origin);
    git(["clone", "-q", origin, checkout], sandbox.home);
    mkdirSync(join(checkout, "scripts"));
    cpSync(
      join(repoRoot, "scripts", "release-version.ts"),
      join(checkout, "scripts", "release-version.ts"),
    );
  }

  const VERSION = /^\d{8}\.[1-9]\d*$/;

  test("a commit after the latest release is released, with that tag as the previous one", () => {
    tags({ "v20261005.1": "c1", "v20261005.2": "c13" });
    const r = step({ DRY_RUN: "false", GITHUB_SHA: sha.c19! });
    expect(r.output).not.toContain("Release skipped");
    expect(r.code).toBe(0);
    expect(r.githubEnv.POLYGLOSS_PREVIOUS_TAG).toBe("v20261005.2");
    expect(r.githubEnv.POLYGLOSS_VERSION).toMatch(VERSION);
    expect(r.githubEnv.SKIP_RELEASE).toBeUndefined();
  });

  test("the first release has no previous tag and is released", () => {
    tags({});
    const r = step({ DRY_RUN: "false", GITHUB_SHA: sha.c3! });
    expect(r.code).toBe(0);
    expect(r.githubEnv.POLYGLOSS_PREVIOUS_TAG).toBe("");
    expect(r.githubEnv.SKIP_RELEASE).toBeUndefined();
  });

  test("a commit the latest release already contains, or is, is skipped with a notice", () => {
    tags({ "v20261005.1": "c1", "v20261005.2": "c13" });
    // c9 is older than the release on c13: a later push released first, or
    // an old run was re-run. c13 is the release itself, re-run.
    for (const key of ["c9", "c13"]) {
      const r = step({ DRY_RUN: "false", GITHUB_SHA: sha[key]! });
      expect({ key, code: r.code }).toEqual({ key, code: 0 });
      expect(r.output).toContain(
        `::notice title=Release skipped::${sha[key]} is not newer than the latest release v20261005.2 (${sha.c13!.slice(0, 7)})`,
      );
      expect(r.githubEnv.SKIP_RELEASE).toBe("true");
      expect(r.githubEnv.POLYGLOSS_PREVIOUS_TAG).toBe("v20261005.2");
    }
  });

  test("a commit beside the latest release, not after it, is skipped", () => {
    tags({ "v20261005.2": "c6" });
    const r = step({ DRY_RUN: "false", GITHUB_SHA: sha.c7! });
    expect(r.code).toBe(0);
    expect(r.githubEnv.SKIP_RELEASE).toBe("true");
  });

  test("a dry run is never skipped", () => {
    tags({ "v20261005.2": "c13" });
    const r = step({ DRY_RUN: "true", GITHUB_SHA: sha.c9! });
    expect(r.code).toBe(0);
    expect(r.output).not.toContain("Release skipped");
    expect(r.githubEnv.SKIP_RELEASE).toBeUndefined();
  });

  test("a release tag or commit the checkout lacks fails the step instead of skipping", () => {
    tags({ "v20261005.2": "c13" });
    // A commit the checkout lacks is a git error too, not "not newer".
    const unknown = step({ DRY_RUN: "false", GITHUB_SHA: "f".repeat(40) });
    expect(unknown.code).not.toBe(0);
    expect(unknown.githubEnv.SKIP_RELEASE).toBeUndefined();
    git(["tag", "-d", "v20261005.2"], checkout);
    const r = step({ DRY_RUN: "false", GITHUB_SHA: sha.c19! });
    expect(r.code).not.toBe(0);
    expect(r.githubEnv.SKIP_RELEASE).toBeUndefined();
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

  test("a section of exactly the limit lists every entry and nothing more", () => {
    for (const [previous, n] of [
      ["v20261005.4", 20],
      [null, 10],
    ] as const) {
      const md = renderNotes({
        version: "20261006.1",
        previous,
        repo: "dakdevs/polygloss",
        commits: feats(n),
        stat,
        sparkle: false,
      });
      const lines = md.split("\n");
      expect(lines.filter((l) => l.startsWith("- feature")).length).toBe(n);
      expect(lines.filter((l) => l.startsWith("- and ")).length).toBe(0);
    }
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
    // An unknown type is a change: its scope stays, the type goes.
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
