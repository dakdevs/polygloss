import { describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { runnerPackages, scenarios } from "../../benches/run-perf";
import { makeSandbox } from "../support/sandbox";

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

function perfJob(): Job {
  const job = loadWorkflow().jobs?.perf;
  if (!job) throw new Error("nightly.yml has no perf job");
  return job;
}

/**
 * Runs the perf job's matrix step as the runner does (`bash -e`), in a
 * stand-in checkout whose benches/baseline.json records `recorded` CPUs, on
 * a runner whose CPU is `cpu`. `sysctl` and `bun` are stubs: `bun` records
 * its arguments and exits with `bunExit`.
 */
function runMatrixStep(opts: {
  cpu: string;
  recorded: string[];
  bunExit?: number;
}): {
  code: number;
  stdout: string;
  bunArgs: string[] | null;
  committed: string;
  candidate: string | null;
} {
  const step = (perfJob().steps ?? []).find((s) =>
    s.run?.includes("benches/run-perf.ts"),
  );
  if (!step?.run) throw new Error("the perf job has no run-perf step");
  const sandbox = makeSandbox();
  try {
    const checkout = join(sandbox.home, "checkout");
    mkdirSync(join(checkout, "benches"), { recursive: true });
    const committed = `${JSON.stringify({
      entries: opts.recorded.map((cpu) => ({
        machine: { cpu, macos: "15.7" },
        git_sha: "0123456789ab",
        date: "2026-10-01",
        rows: [],
      })),
    })}\n`;
    writeFileSync(join(checkout, "benches", "baseline.json"), committed);
    const bin = join(sandbox.home, "bin");
    mkdirSync(bin);
    const stubs = {
      sysctl:
        '[ "$*" = "-n machdep.cpu.brand_string" ] || exit 64\nprintf \'%s\\n\' "$FAKE_CPU"',
      bun: 'printf \'%s\\n\' "$@" > "$BUN_ARGS"\nexit "$FAKE_BUN_EXIT"',
    };
    for (const [name, body] of Object.entries(stubs)) {
      writeFileSync(join(bin, name), `#!/bin/sh\n${body}\n`);
      chmodSync(join(bin, name), 0o755);
    }
    const script = join(sandbox.home, "step.sh");
    writeFileSync(script, step.run);
    const bunArgsFile = join(sandbox.home, "bun-args");
    const r = Bun.spawnSync(["bash", "-e", script], {
      cwd: checkout,
      env: {
        ...sandbox.env,
        PATH: `${bin}:${sandbox.env.PATH ?? ""}`,
        FAKE_CPU: opts.cpu,
        FAKE_BUN_EXIT: String(opts.bunExit ?? 0),
        BUN_ARGS: bunArgsFile,
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    const candidatePath = join(checkout, "benches", "results", "baseline.json");
    return {
      code: r.exitCode,
      stdout: r.stdout.toString(),
      bunArgs: existsSync(bunArgsFile)
        ? readFileSync(bunArgsFile, "utf8")
            .split("\n")
            .filter((a) => a)
        : null,
      committed,
      candidate: existsSync(candidatePath)
        ? readFileSync(candidatePath, "utf8")
        : null,
    };
  } finally {
    sandbox.cleanup();
  }
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

  test("linux-parity fetches the Linux corpus and checks v6.10..v6.11 at 99.9%", () => {
    const job = loadWorkflow().jobs?.["linux-parity"];
    if (!job) throw new Error("nightly.yml has no linux-parity job");
    expect(job["runs-on"]).toBe("macos-15");
    const steps = job.steps ?? [];
    const fetch = steps.find((s) =>
      s.run?.includes("benches/corpora/fetch-linux.sh"),
    );
    const parity = steps.find((s) => s.run?.includes("scripts/git-parity.ts"));
    expect(fetch?.env?.POLYGLOSS_CORPORA).toBe("${{ runner.temp }}/corpora");
    expect(parity?.env?.POLYGLOSS_CORPORA).toBe("${{ runner.temp }}/corpora");
    expect(parity?.env?.POLYGLOSS_CLI_BIN).toBe("target/release/polygloss-cli");
    expect(parity?.run).toMatch(
      /bun scripts\/git-parity\.ts --corpus linux --min-rate 0\.999 --mismatches-dir/,
    );
    const runs = steps.flatMap((s) => (s.run ? [s.run] : [])).join("\n");
    expect(runs).toContain("scripts/cargo.sh build --release -p polygloss-cli");
    expect(runs).not.toMatch(/(^|\s)cargo (build|test|run)/m);
    const upload = steps.find((s) => s.uses === "actions/upload-artifact@v7");
    expect(upload?.if).toBe("always()");
    expect(upload?.with?.["if-no-files-found"]).toBe("ignore");
  });

  test("perf builds every runner the matrix runs, one package per build", () => {
    const runs = (perfJob().steps ?? []).flatMap((s) =>
      s.run ? s.run.split("\n") : [],
    );
    const builds = runs.filter((r) =>
      r.includes("scripts/cargo.sh build --profile perf"),
    );
    // run-perf.ts launches target/perf/polygloss-perf and target/perf/Polygloss
    // (the app's scenarios); each is built alone, as its --build does, so the
    // two packages' features are not unified.
    expect(builds).toEqual([
      "scripts/cargo.sh build --profile perf -p polygloss-perf",
      "scripts/cargo.sh build --profile perf -p polygloss-app",
    ]);
    for (const runner of new Set(
      scenarios.filter((s) => s.enabled).map((s) => s.runner),
    ))
      expect(builds).toContain(
        `scripts/cargo.sh build --profile perf -p ${runnerPackages[runner]}`,
      );
  });

  test("perf makes every corpus and runs the matrix without budgets", () => {
    const job = perfJob();
    expect(job["runs-on"]).toBe("macos-15");
    const steps = job.steps ?? [];
    const runs = steps.flatMap((s) => (s.run ? [s.run] : [])).join("\n");
    expect(runs).toContain("rustup toolchain install");
    for (const script of [
      "bun benches/corpora/make-typical.ts",
      "bun benches/corpora/make-synthetic.ts",
      "bun benches/corpora/make-huge-file.ts",
      "benches/corpora/fetch-linux.sh",
    ])
      expect(runs).toContain(script);
    const matrix = steps.find((s) => s.run?.includes("benches/run-perf.ts"));
    // Budgets are gated on the developer's machine; CI compares against the
    // baseline recorded for its runner class (OQ-P9).
    expect(matrix?.run).toContain(
      "bun benches/run-perf.ts --corpus all --layouts split,unified --compare-baseline",
    );
    expect(matrix?.run).not.toContain("--check-budgets");
    for (const s of steps.filter(
      (s) => s.run?.includes("corpora") || s.run?.includes("run-perf"),
    ))
      expect(s.env?.POLYGLOSS_CORPORA).toBe("${{ runner.temp }}/corpora");
    expect(runs).not.toMatch(/(^|\s)cargo (build|test|run)/m);
    const upload = steps.find((s) => s.uses === "actions/upload-artifact@v7");
    expect(upload?.if).toBe("always()");
    expect(upload?.with?.path).toBe("benches/results");
  });

  test("perf compares with the baseline entry of the runner's CPU", () => {
    const run = runMatrixStep({
      cpu: "Apple M1 (Virtual)",
      recorded: ["Apple M3 Max", "Apple M1 (Virtual)"],
    });
    expect(run.code).toBe(0);
    expect(run.bunArgs).toEqual([
      "benches/run-perf.ts",
      "--corpus",
      "all",
      "--layouts",
      "split,unified",
      "--compare-baseline",
    ]);
    expect(run.stdout).not.toContain("::warning");
    expect(run.candidate).toBeNull();
  });

  test("perf fails the job on a regression against that entry", () => {
    const run = runMatrixStep({
      cpu: "Apple M1 (Virtual)",
      recorded: ["Apple M1 (Virtual)"],
      bunExit: 1,
    });
    expect(run.code).toBe(1);
  });

  test("perf records a candidate baseline and warns when the runner's CPU has no entry", () => {
    // "Apple M1" is another CPU: entries match the whole name, as findBaseline.
    const run = runMatrixStep({
      cpu: "Apple M1 (Virtual)",
      recorded: ["Apple M3 Max", "Apple M1"],
    });
    expect(run.code).toBe(0);
    expect(run.bunArgs).toEqual([
      "benches/run-perf.ts",
      "--corpus",
      "all",
      "--layouts",
      "split,unified",
      "--write-baseline",
      "--baseline",
      "benches/results/baseline.json",
    ]);
    // The candidate starts as the committed file, so committing it keeps
    // every other machine's entry; it lands in the perf-results artifact.
    expect(run.candidate).toBe(run.committed);
    expect(run.stdout).toMatch(/^::warning .*Apple M1 \(Virtual\)/m);
  });

  test("perf still fails the job when a run fails while recording", () => {
    const run = runMatrixStep({
      cpu: "Apple M1 (Virtual)",
      recorded: [],
      bunExit: 1,
    });
    expect(run.code).toBe(1);
    expect(run.bunArgs).toContain("--write-baseline");
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
