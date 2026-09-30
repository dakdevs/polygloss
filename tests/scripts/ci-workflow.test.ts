import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");
const workflowPath = join(repoRoot, ".github", "workflows", "ci.yml");

type Step = { uses?: string; run?: string; with?: Record<string, unknown> };
type Job = { "runs-on"?: string; steps?: Step[] };
type Workflow = { on?: Record<string, unknown>; jobs?: Record<string, Job> };

function loadWorkflow(): Workflow {
  return Bun.YAML.parse(readFileSync(workflowPath, "utf8")) as Workflow;
}

function runs(job: Job): string[] {
  return (job.steps ?? []).flatMap((s) => (s.run ? [s.run] : []));
}

function uses(job: Job, action: string): Step | undefined {
  return (job.steps ?? []).find((s) => s.uses === action);
}

// The command each job exists to run (plan T0.3).
const jobCommands: Record<string, string> = {
  lint: "bun run format:check && bun run lint",
  unit: "scripts/cargo.sh nextest run --workspace --no-tests=warn --profile ci",
  bun: "bun test",
  e2e: "bun run test:e2e",
  // Plan T5.7: crate rules, licenses + advisories, current third-party notices.
  audit:
    "scripts/check-deps.sh && scripts/cargo.sh deny check licenses bans sources advisories && bun scripts/third-party-notices.ts --check",
};

describe("ci workflow", () => {
  test("runs on push and pull_request", () => {
    const wf = loadWorkflow();
    expect(Object.keys(wf.on ?? {}).sort()).toEqual(["pull_request", "push"]);
  });

  test("has exactly the lint, unit, bun, e2e and audit jobs on macos-15", () => {
    const jobs = loadWorkflow().jobs ?? {};
    expect(Object.keys(jobs).sort()).toEqual(Object.keys(jobCommands).sort());
    for (const [name, job] of Object.entries(jobs)) {
      expect({ name, runner: job["runs-on"] }).toEqual({
        name,
        runner: "macos-15",
      });
    }
  });

  test("every job sets up the pinned toolchains before its command", () => {
    for (const [name, job] of Object.entries(loadWorkflow().jobs ?? {})) {
      const steps = job.steps ?? [];
      expect({ name, first: steps[0]?.uses }).toEqual({
        name,
        first: "actions/checkout@v7",
      });
      const commands = runs(job);
      // rustup >= 1.28 no longer installs from rust-toolchain.toml on `rustup show`.
      expect(commands).toContain("rustup toolchain install");
      expect(uses(job, "Swatinem/rust-cache@v2")?.with).toEqual({
        workspaces: ". -> target-shared",
      });
      expect(uses(job, "taiki-e/install-action@v2")?.with).toEqual({
        tool: "cargo-nextest,cargo-deny",
      });
      expect(uses(job, "oven-sh/setup-bun@v2")?.with).toEqual({
        "bun-version": "1.3.14",
      });
      expect(commands).toContain("bun install --frozen-lockfile");
      expect({ name, last: commands[commands.length - 1] }).toEqual({
        name,
        last: jobCommands[name],
      });
    }
  });

  test("never calls Homebrew cargo directly", () => {
    for (const [name, job] of Object.entries(loadWorkflow().jobs ?? {})) {
      for (const cmd of runs(job)) {
        expect({ name, cmd, bare: /(^|[\s;&|])cargo\s/.test(cmd) }).toEqual({
          name,
          cmd,
          bare: false,
        });
      }
    }
  });

  test("audit keeps vulnerabilities fatal and scopes unmaintained to direct deps", () => {
    const deny = Bun.TOML.parse(
      readFileSync(join(repoRoot, "deny.toml"), "utf8"),
    ) as { advisories?: Record<string, unknown> };
    // No blanket ignores; transitive gpui-kit crates are the only unmaintained ones.
    expect(deny.advisories).toEqual({ unmaintained: "workspace" });
  });
});

describe("contributor docs", () => {
  for (const doc of ["README.md", "AGENTS.md"]) {
    test(`${doc} relative links resolve`, () => {
      const path = join(repoRoot, doc);
      expect(existsSync(path)).toBe(true);
      const text = readFileSync(path, "utf8");
      const targets = [...text.matchAll(/\]\(([^)\s]+)\)/g)]
        .map((m) => m[1] ?? "")
        .filter((t) => !/^[a-z]+:/.test(t) && !t.startsWith("#"))
        .map((t) => t.split("#")[0] ?? "");
      expect(targets.length).toBeGreaterThan(0);
      const missing = targets.filter(
        (t) => !existsSync(join(dirname(path), t)),
      );
      expect(missing).toEqual([]);
    });
  }

  test("README documents the cargo wrapper and the completion commands", () => {
    const readme = readFileSync(join(repoRoot, "README.md"), "utf8");
    for (const needle of [
      "scripts/cargo.sh",
      "bun install --frozen-lockfile",
      "bun run lint",
      "bun run test:unit",
      "bun test",
      "bun run test:e2e",
      "MIT OR Apache-2.0",
    ]) {
      expect({ needle, found: readme.includes(needle) }).toEqual({
        needle,
        found: true,
      });
    }
  });

  test("AGENTS.md states the non-negotiable rules", () => {
    const agents = readFileSync(join(repoRoot, "AGENTS.md"), "utf8");
    for (const needle of [
      "docs/plan.md#global-constraints",
      "scripts/cargo.sh",
      "HOME",
      "GPL",
      "bun run test:e2e",
    ]) {
      expect({ needle, found: agents.includes(needle) }).toEqual({
        needle,
        found: true,
      });
    }
  });
});
