// Perf matrix runner (plan T2.9, design §12.1): statistics, budget checks,
// baseline comparison and the CLI, driven here by a fake harness binary so
// no GPUI window opens during `bun test`.
import { afterAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import {
  type Baseline,
  type Row,
  aggregate,
  budgetsPassed,
  checkBudgets,
  compareBaseline,
  findBaseline,
  formatTable,
  loadBudgets,
  percentile,
  planRuns,
  withBaseline,
} from "../../benches/run-perf";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

const budgets = loadBudgets();
const machine = { cpu: "Apple M3 Max", macos: "26.0" };

function row(
  corpus: Row["corpus"],
  layout: Row["layout"],
  metrics: Row["metrics"],
): Row {
  return { corpus, layout, metrics };
}

describe("statistics", () => {
  test("p95 of a known sample", () => {
    const hundred = Array.from({ length: 100 }, (_, i) => i + 1);
    // Nearest rank: the smallest value with at least 95% of the sample at
    // or below it. Order does not matter.
    expect(percentile(hundred, 95)).toBe(95);
    expect(percentile([...hundred].reverse(), 95)).toBe(95);
    expect(percentile(hundred, 50)).toBe(50);
    expect(percentile(hundred, 100)).toBe(100);
    // 20 stops: the 19th smallest.
    const twenty = Array.from({ length: 20 }, (_, i) => (i + 1) * 10);
    expect(percentile(twenty, 95)).toBe(190);
    expect(percentile([7], 95)).toBe(7);
    expect(percentile([], 95)).toBeNull();
  });

  test("repeated runs aggregate to the p95 of their values, ignoring nulls", () => {
    expect(aggregate([250])).toBe(250);
    expect(aggregate([250, 310, 280])).toBe(310);
    expect(aggregate([null, 120, null])).toBe(120);
    expect(aggregate([null, null])).toBeNull();
  });
});

describe("budgets", () => {
  test("budgets.json holds the design §12.1 budgets per corpus", () => {
    expect(budgets.metrics.first_paint_ms.budget).toEqual({
      typical: 300,
      linux: 2000,
    });
    for (const corpus of ["typical", "synthetic", "huge-file", "linux"])
      expect(budgets.metrics.scroll_p95_ms.budget).toHaveProperty(corpus, 8.3);
    for (const corpus of ["typical", "synthetic", "huge-file", "linux"])
      expect(budgets.metrics.highlight_ms.budget).toHaveProperty(corpus, 100);
    expect(budgets.metrics.comment_repaint_ms.budget).toEqual({
      typical: 50,
      synthetic: 50,
    });
    expect(budgets.metrics.watcher_banner_ms.budget).toEqual({ typical: 500 });
    expect(budgets.metrics.peak_rss_mb.budget).toEqual({ linux: 1536 });
  });

  test("budget check flags a miss", () => {
    const checks = checkBudgets(
      [
        row("typical", "split", { scroll_p95_ms: 9.1, first_paint_ms: 120 }),
        row("linux", "unified", { scroll_p95_ms: 8.3, peak_rss_mb: 900 }),
        row("synthetic", "split", { scroll_p95_ms: 8.29 }),
      ],
      budgets,
    );
    const misses = checks.filter((c) => c.status === "miss");
    // Budgets are strict: a value equal to its budget misses it.
    expect(misses.map((c) => [c.corpus, c.layout, c.metric, c.value])).toEqual([
      ["typical", "split", "scroll_p95_ms", 9.1],
      ["linux", "unified", "scroll_p95_ms", 8.3],
    ]);
    expect(
      checks.find(
        (c) => c.corpus === "synthetic" && c.metric === "scroll_p95_ms",
      )?.status,
    ).toBe("pass");
    expect(budgetsPassed(checks)).toBe(false);
    // Only corpora with a budget are checked: no first paint budget on
    // synthetic, no RSS budget on typical.
    expect(
      checks.some(
        (c) => c.corpus === "synthetic" && c.metric === "first_paint_ms",
      ),
    ).toBe(false);
    expect(
      checks.some((c) => c.corpus === "typical" && c.metric === "peak_rss_mb"),
    ).toBe(false);
  });

  test("missing metric null is reported not failed", () => {
    const rows = [
      row("typical", "split", {
        first_paint_ms: 120,
        watcher_banner_ms: null,
      }),
    ];
    const checks = checkBudgets(rows, budgets);
    const banner = checks.find((c) => c.metric === "watcher_banner_ms");
    expect(banner?.status).toBe("missing");
    expect(banner?.value).toBeNull();
    expect(budgetsPassed(checks)).toBe(true);
    const table = formatTable(rows, checks);
    expect(table).toContain("n/a");
    expect(table).not.toContain("✗");
  });

  test("the table shows values with their verdicts", () => {
    const rows = [
      row("typical", "split", {
        first_paint_ms: 120.44,
        scroll_p95_ms: 9.1,
        frame_interval_p95_ms: 8.4,
        frame_interval_max_ms: 33.3,
      }),
    ];
    const table = formatTable(rows, checkBudgets(rows, budgets));
    expect(table).toContain("120.4 ✓");
    expect(table).toContain("9.1 ✗");
    // Frame intervals are reported next to the scroll p95.
    expect(table).toContain("8.4 / 33.3");
  });
});

describe("baseline", () => {
  const base: Baseline = {
    entries: [
      {
        machine,
        git_sha: "abc1234",
        date: "2026-09-29",
        rows: [
          row("typical", "split", {
            first_paint_ms: 100,
            scroll_p95_ms: 2,
            watcher_banner_ms: null,
          }),
        ],
      },
    ],
  };

  test("baseline regression over 10 percent fails", () => {
    const entry = findBaseline(base, machine);
    expect(entry).not.toBeNull();
    const regressions = compareBaseline(
      [
        row("typical", "split", {
          first_paint_ms: 111,
          scroll_p95_ms: 2.2,
          watcher_banner_ms: 400,
        }),
      ],
      entry!,
      budgets,
    );
    // 111 vs 100 is over 10%; 2.2 vs 2 is exactly 10%, which is allowed; a
    // metric the baseline did not measure cannot regress.
    expect(regressions).toEqual([
      {
        corpus: "typical",
        layout: "split",
        metric: "first_paint_ms",
        baseline: 100,
        value: 111,
      },
    ]);
    expect(
      compareBaseline(
        [row("typical", "split", { first_paint_ms: 60, scroll_p95_ms: 1 })],
        entry!,
        budgets,
      ),
    ).toEqual([]);
  });

  test("a baseline is looked up by the machine's CPU", () => {
    expect(findBaseline(base, { cpu: "Apple M1", macos: "26.0" })).toBeNull();
    expect(findBaseline(base, { cpu: machine.cpu, macos: "27.0" })).toBe(
      base.entries[0]!,
    );
  });

  test("writing a baseline replaces this machine's entry only", () => {
    const other = { cpu: "Apple M1", macos: "15.6" };
    const withOther = withBaseline(base, {
      machine: other,
      git_sha: "def5678",
      date: "2026-09-30",
      rows: [],
    });
    expect(withOther.entries.map((e) => e.machine.cpu)).toEqual([
      "Apple M3 Max",
      "Apple M1",
    ]);
    const replaced = withBaseline(withOther, {
      machine,
      git_sha: "fff0000",
      date: "2026-10-01",
      rows: [],
    });
    expect(replaced.entries.map((e) => e.git_sha)).toEqual([
      "fff0000",
      "def5678",
    ]);
  });

  test("the committed baseline parses", () => {
    const committed = JSON.parse(
      readFileSync(join(repoRoot, "benches", "baseline.json"), "utf8"),
    ) as Baseline;
    expect(Array.isArray(committed.entries)).toBe(true);
  });
});

describe("plan", () => {
  test("scenarios run on the corpora their metrics are budgeted for, in both layouts", () => {
    const runs = planRuns({
      corpora: ["typical", "synthetic", "huge-file", "linux"],
      layouts: ["split", "unified"],
      budgets,
    });
    const where = (scenario: string) =>
      runs
        .filter((r) => r.scenario === scenario)
        .map((r) => `${r.corpus}/${r.layout}`);
    expect(where("open")).toEqual([
      "typical/split",
      "typical/unified",
      "linux/split",
      "linux/unified",
    ]);
    expect(where("scroll")).toHaveLength(8);
    expect(where("highlight")).toHaveLength(8);
    expect(where("blocks")).toEqual([
      "typical/split",
      "typical/unified",
      "synthetic/split",
      "synthetic/unified",
    ]);
    // App-level scenarios are planned but not run until the app has them.
    const banner = runs.filter((r) => r.scenario === "watcher-banner");
    expect(banner.map((r) => [r.corpus, r.enabled])).toEqual([
      ["typical", false],
      ["typical", false],
    ]);
  });

  test("a scenario filter and a corpus subset narrow the plan", () => {
    const runs = planRuns({
      corpora: ["huge-file"],
      layouts: ["unified"],
      scenarios: ["scroll", "open"],
      budgets,
    });
    expect(runs.map((r) => `${r.scenario}:${r.corpus}/${r.layout}`)).toEqual([
      "scroll:huge-file/unified",
    ]);
  });
});

// ---- CLI, with a fake harness and tiny stand-in corpora ----

/** git in `repo` with the sandbox's identity and config. */
function git(repo: string, args: string[]): void {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env: sandbox.env });
  if (r.exitCode !== 0)
    throw new Error(`git ${args.join(" ")}: ${r.stderr.toString()}`);
}

/** A two-commit repo tagged `base` and `head`. */
function tinyRepo(dir: string, base: string, head: string): void {
  mkdirSync(dir, { recursive: true });
  git(dir, ["init", "-q"]);
  writeFileSync(join(dir, "a.txt"), "one\n");
  git(dir, ["add", "-A"]);
  git(dir, ["commit", "-q", "-m", "base"]);
  git(dir, ["tag", base]);
  writeFileSync(join(dir, "a.txt"), "two\n");
  git(dir, ["commit", "-q", "-am", "head"]);
  git(dir, ["tag", head]);
}

const corporaRoot = join(sandbox.home, "corpora");
for (const name of ["typical", "synthetic", "huge-file"])
  tinyRepo(join(corporaRoot, "generated", name), "corpus-base", "corpus-head");
tinyRepo(join(corporaRoot, "linux"), "v6.10", "v6.11");

// Prints the result its arguments ask for. The runner gives every run a
// sandboxed environment, so the fake reads its knobs from a control file:
// `SLOW_SCROLL` makes the scroll scenario miss its budget, `FAIL_SCENARIO`
// makes that scenario exit 3. Arguments are echoed into `info` so the tests
// can check what the runner passed.
const fakeHarness = join(sandbox.home, "fake-perf");
const control = join(sandbox.home, "fake-perf.env");
const launches = join(sandbox.home, "fake-perf.log");
writeFileSync(
  fakeHarness,
  `#!/bin/sh
[ -f "${control}" ] && . "${control}"
echo "$*" >> "${launches}"
if [ "$1" = "--version" ]; then echo "fake-perf 0"; exit \${VERSION_EXIT:-0}; fi
corpus= layout= scenario= repo= base= head= mode=three-dot
while [ $# -gt 0 ]; do
  case "$1" in
    --corpus) corpus=$2; shift ;;
    --layout) layout=$2; shift ;;
    --scenario) scenario=$2; shift ;;
    --repo) repo=$2; shift ;;
    --base) base=$2; shift ;;
    --head) head=$2; shift ;;
    --direct) mode=direct ;;
    --json) ;;
    *) echo "fake-perf: unexpected $1" >&2; exit 2 ;;
  esac
  shift
done
sleep 0.2
if [ "$scenario" = "\${FAIL_SCENARIO:-}" ]; then
  echo "fake-perf: $scenario failed" >&2
  exit 3
fi
case "$scenario" in
  open) metrics='"first_paint_ms": 120' ;;
  scroll) metrics="\\"scroll_p95_ms\\": \${SLOW_SCROLL:-2.5}, \\"frame_interval_p95_ms\\": 8.4, \\"frame_interval_max_ms\\": 16.9" ;;
  highlight) metrics='"highlight_ms": 40' ;;
  blocks) metrics='"comment_repaint_ms": 12' ;;
esac
printf '{"scenario":"%s","corpus":"%s","layout":"%s","metrics":{%s},"info":{"repo":"%s","base":"%s","head":"%s","mode":"%s","home":"%s"}}\\n' \\
  "$scenario" "$corpus" "$layout" "$metrics" "$repo" "$base" "$head" "$mode" "$HOME"
`,
);
chmodSync(fakeHarness, 0o755);

/** Runs `run-perf.ts` against the fake; `knobs` go to its control file. */
function runPerf(
  args: string[],
  knobs: Record<string, string> = {},
  extra: Record<string, string> = {},
): { code: number | null; stdout: string; stderr: string } {
  writeFileSync(
    control,
    Object.entries(knobs)
      .map(([k, v]) => `${k}=${v}\n`)
      .join(""),
  );
  const r = Bun.spawnSync(
    ["bun", "benches/run-perf.ts", "--bin", fakeHarness, ...args],
    {
      cwd: repoRoot,
      env: { ...sandbox.env, POLYGLOSS_CORPORA: corporaRoot, ...extra },
    },
  );
  return {
    code: r.exitCode,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

type Results = {
  machine: { cpu: string; macos: string };
  git_sha: string;
  rows: Row[];
  runs: Array<{
    scenario: string;
    corpus: string;
    layout: string;
    peak_rss_mb: number | null;
    result: { info: Record<string, string> } | null;
  }>;
  budgets?: Array<{ metric: string; status: string }>;
};

// Each fake run lives ~0.2 s; the full matrix is 24 of them.
const cliTimeout = 60_000;

describe("run-perf CLI", () => {
  test(
    "runs the matrix, samples RSS and writes the results file",
    () => {
      const out = join(sandbox.home, "results", "all.json");
      const r = runPerf([
        "--corpus",
        "all",
        "--layouts",
        "split,unified",
        "--check-budgets",
        "--out",
        out,
      ]);
      expect({ code: r.code, stderr: r.stderr }).toMatchObject({ code: 0 });
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.rows).toHaveLength(8);
      const typical = results.rows.find(
        (x) => x.corpus === "typical" && x.layout === "split",
      )!;
      expect(typical.metrics).toMatchObject({
        first_paint_ms: 120,
        scroll_p95_ms: 2.5,
        highlight_ms: 40,
        comment_repaint_ms: 12,
        watcher_banner_ms: null,
      });
      expect(typical.metrics.peak_rss_mb).toBeGreaterThan(0);
      // Linux is compared directly (the manifest's mode), each run in its own
      // sandboxed HOME.
      const linuxOpen = results.runs.find(
        (x) => x.scenario === "open" && x.corpus === "linux",
      )!;
      expect(linuxOpen.result?.info).toMatchObject({
        repo: join(corporaRoot, "linux"),
        base: "v6.10",
        head: "v6.11",
        mode: "direct",
      });
      expect(linuxOpen.result?.info.home).not.toBe(process.env.HOME);
      expect(results.machine.cpu.length).toBeGreaterThan(0);
      expect(results.git_sha).toMatch(/^[0-9a-f]{7,}/);
      expect(r.stdout).toContain("typical");
      expect(r.stdout).toContain("n/a");
    },
    cliTimeout,
  );

  test(
    "exits 1 and names the miss when a budget is missed",
    () => {
      const out = join(sandbox.home, "results", "miss.json");
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--scenarios",
          "scroll",
          "--check-budgets",
          "--out",
          out,
        ],
        { SLOW_SCROLL: "9.5" },
      );
      expect(r.code).toBe(1);
      expect(r.stderr).toContain("scroll_p95_ms");
      expect(r.stdout).toContain("9.5 ✗");
      expect(existsSync(out)).toBe(true);
    },
    cliTimeout,
  );

  test(
    "without --check-budgets a miss is shown but does not fail",
    () => {
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--scenarios",
          "scroll",
          "--out",
          join(sandbox.home, "results", "report.json"),
        ],
        { SLOW_SCROLL: "9.5" },
      );
      expect(r.code).toBe(0);
      expect(r.stdout).toContain("9.5 ✗");
    },
    cliTimeout,
  );

  test(
    "compares against this machine's baseline and writes one",
    () => {
      const baseline = join(sandbox.home, "baseline.json");
      writeFileSync(baseline, JSON.stringify({ entries: [] }));
      const args = [
        "--corpus",
        "typical",
        "--layouts",
        "split",
        "--scenarios",
        "scroll",
        "--baseline",
        baseline,
      ];
      const out = (n: string) => ["--out", join(sandbox.home, "results", n)];
      // No entry for this machine: reported, not failed.
      const none = runPerf([...args, "--compare-baseline", ...out("b0.json")]);
      expect(none.code).toBe(0);
      expect(none.stderr).toContain("no baseline");
      const written = runPerf([...args, "--write-baseline", ...out("b1.json")]);
      expect(written.code).toBe(0);
      const stored = JSON.parse(readFileSync(baseline, "utf8")) as Baseline;
      expect(stored.entries).toHaveLength(1);
      expect(stored.entries[0]!.rows[0]!.metrics.scroll_p95_ms).toBe(2.5);
      // 2.5 → 3.0 is a 20% regression.
      const slower = runPerf(
        [...args, "--compare-baseline", ...out("b2.json")],
        {
          SLOW_SCROLL: "3.0",
        },
      );
      expect(slower.code).toBe(1);
      expect(slower.stderr).toContain("regress");
      const same = runPerf([...args, "--compare-baseline", ...out("b3.json")]);
      expect(same.code).toBe(0);
    },
    cliTimeout,
  );

  test(
    "refuses to write a baseline from a run that missed a budget",
    () => {
      const baseline = join(sandbox.home, "baseline-miss.json");
      writeFileSync(baseline, JSON.stringify({ entries: [] }));
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--scenarios",
          "scroll",
          "--check-budgets",
          "--write-baseline",
          "--baseline",
          baseline,
          "--out",
          join(sandbox.home, "results", "wb.json"),
        ],
        { SLOW_SCROLL: "9.5" },
      );
      expect(r.code).toBe(1);
      expect(JSON.parse(readFileSync(baseline, "utf8"))).toEqual({
        entries: [],
      });
    },
    cliTimeout,
  );

  test(
    "a failing harness run fails the matrix",
    () => {
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--scenarios",
          "open",
          "--out",
          join(sandbox.home, "results", "fail.json"),
        ],
        { FAIL_SCENARIO: "open" },
      );
      expect(r.code).toBe(1);
      expect(r.stderr).toContain("open");
      expect(r.stderr).toContain("fake-perf: open failed");
    },
    cliTimeout,
  );

  test(
    "launches the harness once before the matrix",
    () => {
      writeFileSync(launches, "");
      const r = runPerf([
        "--corpus",
        "typical",
        "--layouts",
        "split",
        "--scenarios",
        "open",
        "--out",
        join(sandbox.home, "results", "warm.json"),
      ]);
      expect(r.code).toBe(0);
      // The first launch of a freshly built binary pays macOS's code
      // assessment (seconds); it must not land in a measured run.
      const lines = readFileSync(launches, "utf8").trim().split("\n");
      expect(lines[0]).toBe("--version");
      expect(
        lines.slice(1).map((l) => l.split(" ").slice(0, 6).join(" ")),
      ).toEqual(["--corpus typical --layout split --scenario open"]);
    },
    cliTimeout,
  );

  test(
    "a harness that does not start is a setup error",
    () => {
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--out",
          join(sandbox.home, "results", "nostart.json"),
        ],
        { VERSION_EXIT: "1" },
      );
      expect(r.code).toBe(2);
      expect(r.stderr).toContain("does not start");
    },
    cliTimeout,
  );

  test(
    "--repeat runs everything n times and aggregates per metric",
    () => {
      const out = join(sandbox.home, "results", "repeat.json");
      const r = runPerf([
        "--corpus",
        "typical",
        "--layouts",
        "unified",
        "--scenarios",
        "open,blocks",
        "--repeat",
        "2",
        "--out",
        out,
      ]);
      expect(r.code).toBe(0);
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.runs.map((x) => x.scenario)).toEqual([
        "open",
        "blocks",
        "open",
        "blocks",
      ]);
      expect(results.rows).toHaveLength(1);
      expect(results.rows[0]!.metrics).toMatchObject({
        first_paint_ms: 120,
        comment_repaint_ms: 12,
      });
    },
    cliTimeout,
  );

  test(
    "dry run prints the plan without running anything",
    () => {
      const r = runPerf([
        "--corpus",
        "linux",
        "--layouts",
        "split",
        "--dry-run",
      ]);
      expect(r.code).toBe(0);
      expect(r.stdout).toContain("open linux split");
      expect(r.stdout).toContain("--direct");
      expect(r.stdout).not.toContain("blocks");
    },
    cliTimeout,
  );

  test(
    "a missing corpus names the command that creates it",
    () => {
      const r = runPerf(
        ["--corpus", "typical", "--layouts", "split"],
        {},
        { POLYGLOSS_CORPORA: join(sandbox.home, "no-corpora") },
      );
      expect(r.code).toBe(2);
      expect(r.stderr).toContain("bun benches/corpora/make-typical.ts");
    },
    cliTimeout,
  );

  test(
    "usage errors exit 2",
    () => {
      expect(runPerf(["--layouts", "diagonal"]).code).toBe(2);
      expect(runPerf(["--corpus", "windows"]).code).toBe(2);
      expect(runPerf(["--repeat", "0"]).code).toBe(2);
      expect(runPerf(["--frobnicate"]).code).toBe(2);
    },
    cliTimeout,
  );
});
