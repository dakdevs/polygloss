// scripts/release-dry-run.ts (plan T5.8): runs .github/workflows/release.yml's
// steps locally the way GitHub would for one event, with every secret empty,
// and never runs a step that publishes. The step-running tests use small
// throwaway workflows in a sandbox; only --plan and the tag check read the
// real release.yml.
import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  evaluateCondition,
  expandExpressions,
} from "../../scripts/release-dry-run";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts/release-dry-run.ts");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

let n = 0;
/** Writes `yaml` as a workflow in a fresh dir; returns its path and dir. */
function workflow(yaml: string): { path: string; dir: string } {
  const dir = join(sandbox.home, `wf-${n++}`);
  mkdirSync(dir, { recursive: true });
  const path = join(dir, "release.yml");
  writeFileSync(path, yaml);
  return { path, dir };
}

function dryRun(
  args: string[],
  cwd: string = repoRoot,
): { code: number; stdout: string; stderr: string } {
  const r = Bun.spawnSync(["bun", script, ...args], {
    cwd,
    env: { ...sandbox.env, SECRET_FROM_CALLER: "leak" },
    stdout: "pipe",
    stderr: "pipe",
    timeout: 60_000,
  });
  return {
    code: r.exitCode ?? -1,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

/** `--plan --json` decisions as `{name: decision}`. */
function plan(args: string[]): Record<string, string> {
  const r = dryRun(["--plan", "--json", ...args]);
  expect(r.stderr).toBe("");
  expect(r.code).toBe(0);
  const steps = JSON.parse(r.stdout).steps as {
    name: string;
    decision: string;
  }[];
  return Object.fromEntries(steps.map((s) => [s.name, s.decision]));
}

describe("expressions", () => {
  const ctx = {
    event: "workflow_dispatch",
    ref: "refs/heads/main",
    repository: "owner/polygloss",
    runnerTemp: "/tmp/rt",
    vars: { HOMEBREW_TAP_REPO: "owner/homebrew-tap" },
    inputs: { dry_run: "true" },
  };

  test("conditions github would evaluate for the release workflow", () => {
    expect(evaluateCondition(undefined, ctx, false)).toBe(true);
    expect(evaluateCondition(undefined, ctx, true)).toBe(false);
    expect(evaluateCondition("always()", ctx, true)).toBe(true);
    expect(evaluateCondition("failure()", ctx, true)).toBe(true);
    expect(evaluateCondition("failure()", ctx, false)).toBe(false);
    expect(
      evaluateCondition("startsWith(github.ref, 'refs/tags/v')", ctx, false),
    ).toBe(false);
    expect(
      evaluateCondition(
        "startsWith(github.ref, 'refs/tags/v')",
        { ...ctx, event: "push", ref: "refs/tags/v0.1.0" },
        false,
      ),
    ).toBe(true);
    expect(
      evaluateCondition("github.event_name == 'workflow_dispatch'", ctx, false),
    ).toBe(true);
    // Inputs, negated or not; `false` and empty are false.
    expect(evaluateCondition("${{ inputs.dry_run }}", ctx, false)).toBe(true);
    expect(evaluateCondition("${{ !inputs.dry_run }}", ctx, false)).toBe(false);
    const release = { ...ctx, inputs: { dry_run: "false" } };
    expect(evaluateCondition("${{ !inputs.dry_run }}", release, false)).toBe(
      true,
    );
    expect(evaluateCondition("inputs.dry_run", release, false)).toBe(false);
    expect(evaluateCondition("!inputs.unset", ctx, false)).toBe(true);
    // A tag condition never runs after a failure (an implicit success()).
    expect(
      evaluateCondition(
        "startsWith(github.ref, 'refs/tags/v')",
        { ...ctx, event: "push", ref: "refs/tags/v0.1.0" },
        true,
      ),
    ).toBe(false);
  });

  test("secrets and the token are empty, vars and github values are filled in", () => {
    expect(expandExpressions("${{ secrets.APPLE_CERTIFICATE }}", ctx)).toBe("");
    expect(expandExpressions("${{ github.token }}", ctx)).toBe("");
    expect(expandExpressions("${{ vars.HOMEBREW_TAP_REPO }}", ctx)).toBe(
      "owner/homebrew-tap",
    );
    expect(expandExpressions("${{ vars.UNSET }}", ctx)).toBe("");
    expect(expandExpressions("${{ inputs.dry_run }}", ctx)).toBe("true");
    expect(expandExpressions("release-${{ github.ref }}", ctx)).toBe(
      "release-refs/heads/main",
    );
    expect(expandExpressions("${{github.ref_name}}", ctx)).toBe("main");
  });

  test("an expression it does not know fails loudly", () => {
    expect(() => expandExpressions("${{ matrix.os }}", ctx)).toThrow(
      /matrix\.os/,
    );
    expect(() =>
      evaluateCondition("contains(github.ref, 'x')", ctx, false),
    ).toThrow(/contains/);
  });
});

describe("--plan on the real release.yml", () => {
  test("workflow_dispatch is a dry run by default: it builds and keeps the DMG, publishes nothing", () => {
    const p = plan(["--event", "workflow_dispatch"]);
    expect(p["actions/checkout@v7"]).toBe("action");
    for (const step of [
      "Check the release secrets",
      "Compute the version",
      "Write the release notes",
      "Fetch Sparkle",
      "Build, sign and notarize the bundle and DMG",
      "Remove the notarytool API key",
      "Smoke-test the bundle",
    ])
      expect({ step, decision: p[step] }).toEqual({ step, decision: "run" });
    expect(p["actions/upload-artifact@v7"]).toBe("artifact-check");
    for (const step of [
      "Write the notarytool API key",
      "Make the appcast and checksums",
      "Publish the GitHub release",
      "Bump the Homebrew tap",
    ])
      expect({ step, decision: p[step] }).toEqual({
        step,
        decision: "skip-if",
      });
  });

  test("ci.yml's call on main releases: it signs and publishes, which a local run refuses", () => {
    for (const args of [
      ["--event", "push", "--ref", "refs/heads/main"],
      ["--event", "workflow_dispatch", "--input", "dry_run=false"],
    ]) {
      const p = plan(args);
      expect(p["Write the notarytool API key"]).toBe("run");
      expect(p["Make the appcast and checksums"]).toBe("run");
      expect(p["actions/upload-artifact@v7"]).toBe("skip-if");
      expect(p["Publish the GitHub release"]).toBe("refuse-publish");
      expect(p["Bump the Homebrew tap"]).toBe("refuse-publish");
    }
  });
});

describe("running steps", () => {
  test("runs steps in order with empty secrets, filled vars and GITHUB_ENV carried over", () => {
    const wf = workflow(`
name: release
on: { workflow_dispatch: {} }
env:
  TOP: top
jobs:
  release:
    runs-on: macos-15
    env:
      JOB: job
    steps:
      - uses: actions/checkout@v7
      - name: first
        env:
          SECRET: \${{ secrets.APPLE_CERTIFICATE }}
          VAR: \${{ vars.HOMEBREW_TAP_REPO }}
        run: |
          echo "first top=$TOP job=$JOB secret=[$SECRET] var=[$VAR] caller=[\${SECRET_FROM_CALLER-}]"
          echo "CARRIED=yes" >>"$GITHUB_ENV"
          test -d "$RUNNER_TEMP"
      - name: second
        run: echo "second carried=$CARRIED ref=$GITHUB_REF_NAME event=$GITHUB_EVENT_NAME repo=$GITHUB_REPOSITORY"
`);
    const r = dryRun(
      [
        "--workflow",
        wf.path,
        "--var",
        "HOMEBREW_TAP_REPO=me/tap",
        "--ref",
        "refs/heads/dev",
      ],
      wf.dir,
    );
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
    expect(r.stdout).toContain(
      "first top=top job=job secret=[] var=[me/tap] caller=[]",
    );
    expect(r.stdout).toContain(
      "second carried=yes ref=dev event=workflow_dispatch repo=owner/polygloss",
    );
    expect(r.stdout.indexOf("first top")).toBeLessThan(
      r.stdout.indexOf("second carried"),
    );
    expect(r.stdout).toMatch(/action\s+actions\/checkout@v7/);
  });

  test("--env seeds the steps' environment; GITHUB_ENV and step env win over it", () => {
    const wf = workflow(`
on: workflow_dispatch
jobs:
  release:
    steps:
      - name: first
        run: |
          echo "first seeded=$SEEDED over=$OVER"
          echo "OVER=from-github-env" >>"$GITHUB_ENV"
      - name: second
        run: echo "second over=$OVER"
`);
    const r = dryRun(
      ["--workflow", wf.path, "--env", "SEEDED=yes", "--env", "OVER=seed"],
      wf.dir,
    );
    expect(r.code).toBe(0);
    expect(r.stdout).toContain("first seeded=yes over=seed");
    expect(r.stdout).toContain("second over=from-github-env");
    expect(dryRun(["--env", "NOEQUALS", "--plan"]).code).toBe(2);
  });

  test("GITHUB_SHA is the checkout's HEAD, empty outside a git repo", () => {
    const yaml = `
on: workflow_dispatch
jobs:
  release:
    steps:
      - name: sha
        run: echo "sha=[$GITHUB_SHA]"
`;
    const plain = workflow(yaml);
    expect(dryRun(["--workflow", plain.path], plain.dir).stdout).toContain(
      "sha=[]",
    );
    const repo = workflow(yaml);
    const git = (args: string[]) =>
      Bun.spawnSync(["git", ...args], { cwd: repo.dir, env: sandbox.env })
        .stdout.toString()
        .trim();
    git(["init", "-q"]);
    git(["add", "release.yml"]);
    git(["commit", "-q", "-m", "workflow"]);
    const head = git(["rev-parse", "HEAD"]);
    expect(head).toMatch(/^[0-9a-f]{40}$/);
    expect(dryRun(["--workflow", repo.path], repo.dir).stdout).toContain(
      `sha=[${head}]`,
    );
  });

  test("a failing step skips the rest except always() steps and exits 1", () => {
    const wf = workflow(`
on: workflow_dispatch
jobs:
  release:
    steps:
      - name: breaks
        run: |
          false
          echo "not reached"
      - name: later
        run: echo "later ran"
      - name: cleanup
        if: always()
        run: echo "cleanup ran"
`);
    const r = dryRun(["--workflow", wf.path], wf.dir);
    expect(r.code).toBe(1);
    expect(r.stdout).not.toContain("not reached");
    expect(r.stdout).not.toContain("later ran");
    expect(r.stdout).toContain("cleanup ran");
    expect(r.stdout).toMatch(/failed\s+breaks/);
    expect(r.stdout).toMatch(/skip-if\s+later/);
  });

  test("never runs a step that publishes, whatever the event", () => {
    const wf = workflow(`
on: workflow_dispatch
jobs:
  release:
    steps:
      - name: publish
        run: gh release upload v1 dist/x.dmg
      - name: push
        run: |
          echo "pushing"
          git -C tap push
      - name: fine
        run: echo "fine ran"
`);
    const r = dryRun(["--workflow", wf.path], wf.dir);
    expect(r.code).toBe(0);
    expect(r.stdout).not.toContain("pushing");
    expect(r.stdout).toContain("fine ran");
    expect(r.stdout).toMatch(/refuse-publish\s+publish/);
    expect(r.stdout).toMatch(/refuse-publish\s+push/);
  });

  test("upload-artifact checks that its path matches when files are required", () => {
    const yaml = `
on: workflow_dispatch
jobs:
  release:
    steps:
      - uses: actions/upload-artifact@v7
        with:
          name: dmg
          path: dist/*.dmg
          if-no-files-found: error
`;
    const missing = workflow(yaml);
    const r1 = dryRun(["--workflow", missing.path], missing.dir);
    expect(r1.code).toBe(1);
    expect(r1.stdout).toMatch(/failed\s+actions\/upload-artifact@v7/);
    expect(r1.stderr).toContain("no files match dist/*.dmg");

    const present = workflow(yaml);
    mkdirSync(join(present.dir, "dist"));
    writeFileSync(join(present.dir, "dist/Polygloss_0.1.0_aarch64.dmg"), "x");
    const r2 = dryRun(["--workflow", present.path], present.dir);
    expect(r2.code).toBe(0);
    expect(r2.stdout).toContain("dist/Polygloss_0.1.0_aarch64.dmg");
  });

  test("--skip leaves a named step out", () => {
    const wf = workflow(`
on: workflow_dispatch
jobs:
  release:
    steps:
      - run: echo "setup ran"
      - name: main
        run: echo "main ran"
`);
    const r = dryRun(
      ["--workflow", wf.path, "--skip", 'echo "setup ran"'],
      wf.dir,
    );
    expect(r.code).toBe(0);
    expect(r.stdout).not.toContain("setup ran\n");
    expect(r.stdout).toContain("main ran");
    expect(r.stdout).toMatch(/skipped\s+echo "setup ran"/);
  });

  test("usage errors exit 2", () => {
    expect(dryRun(["--event", "schedule", "--plan"]).code).toBe(2);
    expect(dryRun(["--workflow", join(sandbox.home, "none.yml")]).code).toBe(2);
    expect(dryRun(["--only", "No such step", "--plan"]).code).toBe(2);
  });
});
