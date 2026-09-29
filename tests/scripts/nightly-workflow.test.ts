import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");
const workflowPath = join(repoRoot, ".github", "workflows", "nightly.yml");

type Step = {
  name?: string;
  uses?: string;
  run?: string;
  if?: string;
  env?: Record<string, string>;
  with?: Record<string, unknown>;
};
type Job = {
  "runs-on"?: string;
  strategy?: { "fail-fast"?: boolean; matrix?: Record<string, unknown> };
  steps?: Step[];
};
type Workflow = {
  on?: { schedule?: Array<{ cron?: string }> } & Record<string, unknown>;
  jobs?: Record<string, Job>;
};

function loadWorkflow(): Workflow {
  return Bun.YAML.parse(readFileSync(workflowPath, "utf8")) as Workflow;
}

function parityJob(): Job {
  const job = loadWorkflow().jobs?.["git-parity"];
  if (!job) throw new Error("nightly.yml has no git-parity job");
  return job;
}

describe("nightly workflow", () => {
  test("runs on a cron schedule and on demand", () => {
    const on = loadWorkflow().on ?? {};
    expect(on.schedule?.[0]?.cron).toMatch(/^\S+ \S+ \* \* \*$/);
    expect(Object.keys(on)).toContain("workflow_dispatch");
  });

  test("git-parity runs on macOS arm64 for seeds 1 to 5", () => {
    const job = parityJob();
    expect(job["runs-on"]).toBe("macos-15");
    expect(job.strategy?.["fail-fast"]).toBe(false);
    expect(job.strategy?.matrix?.seed).toEqual([1, 2, 3, 4, 5]);
  });

  test("builds the CLI through scripts/cargo.sh and checks 500 files per seed", () => {
    const runs = (parityJob().steps ?? []).flatMap((s) =>
      s.run ? [s.run] : [],
    );
    const all = runs.join("\n");
    expect(all).toContain("rustup toolchain install");
    expect(all).toContain("scripts/cargo.sh build --release -p polygloss-cli");
    expect(all).toContain(
      "bun scripts/make-parity-repo.ts --seed ${{ matrix.seed }} --files 500",
    );
    expect(all).toMatch(
      /bun scripts\/git-parity\.ts .*--range parity-base\.\.parity-head .*--min-rate 0\.999 .*--mismatches-dir/,
    );
    expect(all).not.toMatch(/(^|\s)cargo (build|test|run)/m);
  });

  test("uploads mismatches as an artifact whether or not parity passes", () => {
    const upload = (parityJob().steps ?? []).find(
      (s) => s.uses === "actions/upload-artifact@v7",
    );
    expect(upload?.if).toBe("always()");
    expect(upload?.with?.["if-no-files-found"]).toBe("ignore");
    expect(String(upload?.with?.name)).toContain("${{ matrix.seed }}");
  });
});
