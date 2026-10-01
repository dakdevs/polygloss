// Perf matrix runner (plan T2.9, design §12.1): statistics, budget checks,
// baseline comparison and the CLI, driven here by a fake harness binary so
// no GPUI window opens during `bun test`.
import { afterAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
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
  FRAME_BOUND_METRICS,
  FRAME_NOISE_MS,
  findBaseline,
  formatTable,
  loadBudgets,
  percentile,
  planRuns,
  screenLocked,
  warmupRun,
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

describe("screen lock", () => {
  test("ioreg's CGSSessionScreenIsLocked = Yes means the screen is locked", () => {
    const session = (locked: string) =>
      `    | "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes,${locked}"kCGSSessionUserIDKey"=501})`;
    expect(screenLocked(session('"CGSSessionScreenIsLocked"=Yes,'))).toBe(true);
    expect(screenLocked(session('"CGSSessionScreenIsLocked"=No,'))).toBe(false);
    // The key is absent while the screen is unlocked.
    expect(screenLocked(session(""))).toBe(false);
    expect(screenLocked("")).toBe(false);
  });
});

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
    // The same budget, measured in the app itself (T3.1).
    expect(budgets.metrics.app_first_paint_ms.budget).toEqual({
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

  test("frame-bound metrics regress only past 10 percent and one 120 Hz frame (OQ-P18)", () => {
    const entry = {
      machine,
      git_sha: "abc1234",
      date: "2026-09-29",
      rows: [
        row("huge-file", "unified", { highlight_ms: 9.8, scroll_p95_ms: 1 }),
        row("typical", "split", {
          highlight_ms: 15.4,
          comment_repaint_ms: 11.5,
          watcher_banner_ms: 100,
          app_first_paint_ms: 200,
        }),
      ],
    };
    const regressions = compareBaseline(
      [
        // +2.2 ms (22%): one frame of noise, not a regression. scroll_p95_ms
        // is CPU time per frame, not frame-bound: 10% is enough.
        row("huge-file", "unified", { highlight_ms: 12.0, scroll_p95_ms: 1.2 }),
        row("typical", "split", {
          // +8.3 ms exactly: still within one frame.
          highlight_ms: 23.7,
          // +13.5 ms and over 10%: a regression.
          comment_repaint_ms: 25,
          // +9 ms but under 10%: not a regression.
          watcher_banner_ms: 109,
          // +30 ms and 15%: a regression.
          app_first_paint_ms: 230,
        }),
      ],
      entry,
      budgets,
    );
    expect(regressions.map((r) => `${r.corpus}/${r.metric}`)).toEqual([
      "huge-file/scroll_p95_ms",
      "typical/comment_repaint_ms",
      "typical/app_first_paint_ms",
    ]);
    expect(FRAME_NOISE_MS).toBe(8.3);
    expect(FRAME_BOUND_METRICS).not.toContain("scroll_p95_ms");
    expect(FRAME_BOUND_METRICS).not.toContain("peak_rss_mb");
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
    // Comment round-trips run in the app (T3.10) where their budget
    // applies; polygloss-perf's M2 `blocks` is no longer planned.
    expect(where("comment-roundtrip")).toEqual([
      "typical/split",
      "typical/unified",
      "synthetic/split",
      "synthetic/unified",
    ]);
    expect(
      runs
        .filter((r) => r.scenario === "comment-roundtrip")
        .every((r) => r.runner === "app" && r.enabled),
    ).toBe(true);
    expect(where("blocks")).toEqual([]);
    // The app's own first paint runs in the app (`Polygloss --perf-scenario
    // open`) where its budget applies.
    expect(where("app-open")).toEqual([
      "typical/split",
      "typical/unified",
      "linux/split",
      "linux/unified",
    ]);
    expect(
      runs
        .filter((r) => r.scenario === "app-open")
        .every((r) => r.runner === "app" && r.enabled),
    ).toBe(true);
    // The watcher→banner time runs in the app (T3.11) on the typical
    // corpus, where its budget applies.
    const banner = runs.filter((r) => r.scenario === "watcher-banner");
    expect(banner.map((r) => [r.corpus, r.runner, r.enabled])).toEqual([
      ["typical", "app", true],
      ["typical", "app", true],
    ]);
  });

  test("the warm-up opens the first planned corpus in the first planned layout", () => {
    const plan = planRuns({
      corpora: ["huge-file", "linux"],
      layouts: ["unified"],
      scenarios: ["highlight"],
      budgets,
    });
    expect(warmupRun(plan)).toEqual({
      scenario: "open",
      runner: "perf",
      corpus: "huge-file",
      layout: "unified",
      metrics: ["first_paint_ms"],
      enabled: true,
    });
    // Nothing runs, nothing to warm up.
    const none = planRuns({
      corpora: ["huge-file"],
      layouts: ["split"],
      scenarios: ["watcher-banner"],
      budgets,
    });
    expect(none).toEqual([]);
    expect(warmupRun(none)).toBeNull();
    // An app-only plan warms up the app, not polygloss-perf.
    const banner = planRuns({
      corpora: ["typical"],
      layouts: ["split"],
      scenarios: ["watcher-banner"],
      budgets,
    });
    expect(warmupRun(banner)).toBeNull();
    expect(warmupRun(banner, "app")).toMatchObject({
      scenario: "app-open",
      corpus: "typical",
      layout: "split",
    });
    // Each runner warms up its own binary: the app has its own Metal shader
    // cache (OQ-P12).
    const both = planRuns({
      corpora: ["linux"],
      layouts: ["unified"],
      scenarios: ["app-open", "scroll"],
      budgets,
    });
    expect(warmupRun(both, "perf")).toMatchObject({
      scenario: "open",
      runner: "perf",
      corpus: "linux",
      layout: "unified",
    });
    expect(warmupRun(both, "app")).toEqual({
      scenario: "app-open",
      runner: "app",
      corpus: "linux",
      layout: "unified",
      metrics: ["app_first_paint_ms"],
      enabled: true,
    });
    expect(warmupRun(none, "app")).toBeNull();
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
// makes that scenario exit 3, `COLD_FIRST_PAINT` is the first paint of the
// first windowed launch while `firstWindow` does not exist (the launch
// creates it), like a fresh build compiling GPUI's shaders once. Arguments
// are echoed into `info` so the tests can check what the runner passed.
const fakeHarness = join(sandbox.home, "fake-perf");
const control = join(sandbox.home, "fake-perf.env");
const launches = join(sandbox.home, "fake-perf.log");
const firstWindow = join(sandbox.home, "fake-perf.shaders");
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
first_paint=120
if [ -n "\${COLD_FIRST_PAINT:-}" ] && [ ! -f "${firstWindow}" ]; then
  : > "${firstWindow}"
  first_paint=$COLD_FIRST_PAINT
fi
case "$scenario" in
  open) metrics="\\"first_paint_ms\\": $first_paint" ;;
  scroll) metrics="\\"scroll_p95_ms\\": \${SLOW_SCROLL:-2.5}, \\"frame_interval_p95_ms\\": 8.4, \\"frame_interval_max_ms\\": 16.9" ;;
  highlight) metrics='"highlight_ms": 40' ;;
  blocks) metrics='"comment_repaint_ms": 12' ;;
esac
printf '{"scenario":"%s","corpus":"%s","layout":"%s","metrics":{%s},"info":{"repo":"%s","base":"%s","head":"%s","mode":"%s","home":"%s"}}\\n' \\
  "$scenario" "$corpus" "$layout" "$metrics" "$repo" "$base" "$head" "$mode" "$HOME"
`,
);
chmodSync(fakeHarness, 0o755);

// The app's side (`Polygloss --perf-scenario <name> …`, T3.1): it needs
// POLYGLOSS_TEST=1 like the real app and echoes what it was given.
const fakeApp = join(sandbox.home, "fake-app");
const appLaunches = join(sandbox.home, "fake-app.log");
writeFileSync(
  fakeApp,
  `#!/bin/sh
[ -f "${control}" ] && . "${control}"
echo "$*" >> "${appLaunches}"
if [ "$1" = "--version" ]; then echo "Polygloss 0"; exit 0; fi
if [ "$1" != "--perf-scenario" ]; then echo "fake-app: expected --perf-scenario" >&2; exit 2; fi
if [ "$POLYGLOSS_TEST" != "1" ]; then echo "fake-app: set POLYGLOSS_TEST=1" >&2; exit 2; fi
scenario=$2; shift 2
corpus= layout= repo= base= head= mode=three-dot
while [ $# -gt 0 ]; do
  case "$1" in
    --corpus) corpus=$2; shift ;;
    --layout) layout=$2; shift ;;
    --repo) repo=$2; shift ;;
    --base) base=$2; shift ;;
    --head) head=$2; shift ;;
    --direct) mode=direct ;;
    --json) ;;
    *) echo "fake-app: unexpected $1" >&2; exit 2 ;;
  esac
  shift
done
sleep 0.2
case "$scenario" in
  open) metrics="\\"app_first_paint_ms\\": \${APP_FIRST_PAINT:-150}" ;;
  watcher-banner) metrics="\\"watcher_banner_ms\\": \${WATCHER_BANNER:-260}" ;;
  comment-roundtrip) metrics='"comment_repaint_ms": 12' ;;
  *) echo "fake-app: unknown scenario $scenario" >&2; exit 2 ;;
esac
printf '{"scenario":"%s","corpus":"%s","layout":"%s","metrics":{%s},"info":{"repo":"%s","base":"%s","head":"%s","mode":"%s","polygloss_test":"%s"}}\\n' \\
  "$scenario" "$corpus" "$layout" "$metrics" "$repo" "$base" "$head" "$mode" "$POLYGLOSS_TEST"
`,
);
chmodSync(fakeApp, 0o755);

// `ioreg -n Root -d1` (POLYGLOSS_IOREG): the session's lock state comes from
// FAKE_SCREEN_LOCKED, so the CLI tests do not depend on the real screen.
const fakeIoreg = join(sandbox.home, "fake-ioreg");
writeFileSync(
  fakeIoreg,
  `#!/bin/sh
[ "$*" = "-n Root -d1" ] || { echo "fake-ioreg: unexpected $*" >&2; exit 2; }
echo '  | "IOConsoleUsers" = ({"kCGSSessionOnConsoleKey"=Yes,"CGSSessionScreenIsLocked"='"\${FAKE_SCREEN_LOCKED:-No}"'})'
`,
);
chmodSync(fakeIoreg, 0o755);

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
    [
      "bun",
      "benches/run-perf.ts",
      "--bin",
      fakeHarness,
      "--app-bin",
      fakeApp,
      ...args,
    ],
    {
      cwd: repoRoot,
      env: {
        ...sandbox.env,
        POLYGLOSS_CORPORA: corporaRoot,
        POLYGLOSS_IOREG: fakeIoreg,
        ...extra,
      },
    },
  );
  return {
    code: r.exitCode,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

type RunResult = {
  scenario: string;
  corpus: string;
  layout: string;
  peak_rss_mb: number | null;
  result: {
    metrics: Record<string, number | null>;
    info: Record<string, string>;
  } | null;
  error: string | null;
};
type Results = {
  machine: { cpu: string; macos: string };
  git_sha: string;
  rows: Row[];
  warmup: RunResult | null;
  runs: RunResult[];
  budgets?: Array<{ metric: string; status: string }>;
};

// Each fake run lives ~0.2 s; the full matrix is 30 of them.
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
        watcher_banner_ms: 260,
        app_first_paint_ms: 150,
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
      // The watcher→banner time comes from the app (T3.11).
      expect(r.stdout).toContain("260 ✓");
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
    "warms the harness up before the matrix: a launch, then one unmeasured window",
    () => {
      writeFileSync(launches, "");
      const out = join(sandbox.home, "results", "warm.json");
      const r = runPerf([
        "--corpus",
        "synthetic",
        "--layouts",
        "unified",
        "--scenarios",
        "scroll",
        "--out",
        out,
      ]);
      expect(r.code).toBe(0);
      // The first launch of a freshly built binary pays macOS's code
      // assessment (seconds) and its first window compiles GPUI's shaders
      // (OQ-P12); neither may land in a measured run. The warm-up window is
      // an `open` of the first planned corpus and layout.
      const lines = readFileSync(launches, "utf8").trim().split("\n");
      expect(lines[0]).toBe("--version");
      expect(
        lines.slice(1).map((l) => l.split(" ").slice(0, 6).join(" ")),
      ).toEqual([
        "--corpus synthetic --layout unified --scenario open",
        "--corpus synthetic --layout unified --scenario scroll",
      ]);
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.runs.map((x) => x.scenario)).toEqual(["scroll"]);
      expect(results.warmup).toMatchObject({
        scenario: "open",
        corpus: "synthetic",
        layout: "unified",
        error: null,
      });
    },
    cliTimeout,
  );

  test(
    "a fresh binary's slower first window is kept out of the matrix and reported",
    () => {
      rmSync(firstWindow, { force: true });
      const out = join(sandbox.home, "results", "cold.json");
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--scenarios",
          "open",
          "--check-budgets",
          "--out",
          out,
        ],
        { COLD_FIRST_PAINT: "380" },
      );
      expect({ code: r.code, stderr: r.stderr }).toMatchObject({ code: 0 });
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.rows[0]!.metrics.first_paint_ms).toBe(120);
      expect(results.warmup?.result?.metrics.first_paint_ms).toBe(380);
      expect(r.stderr).toContain("warm-up");
      expect(r.stderr).toContain("first paint 380");
    },
    cliTimeout,
  );

  test(
    "a failed warm-up is reported and the matrix still runs",
    () => {
      const out = join(sandbox.home, "results", "warm-fail.json");
      const r = runPerf(
        [
          "--corpus",
          "typical",
          "--layouts",
          "split",
          "--scenarios",
          "scroll",
          "--out",
          out,
        ],
        { FAIL_SCENARIO: "open" },
      );
      expect(r.code).toBe(0);
      expect(r.stderr).toContain("warm-up");
      expect(r.stderr).toContain("failed");
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.warmup?.error).toContain("exited with code 3");
      expect(results.rows[0]!.metrics.scroll_p95_ms).toBe(2.5);
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
        "open,comment-roundtrip",
        "--repeat",
        "2",
        "--out",
        out,
      ]);
      expect(r.code).toBe(0);
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.runs.map((x) => x.scenario)).toEqual([
        "open",
        "comment-roundtrip",
        "open",
        "comment-roundtrip",
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
      expect(r.stdout).not.toContain("comment-roundtrip");
      expect(r.stdout.split("\n")[0]).toMatch(
        /^warm-up \(not measured\): open linux split: .* --scenario open /,
      );
    },
    cliTimeout,
  );

  test(
    "a locked screen stops the run before any window opens",
    () => {
      rmSync(appLaunches, { force: true });
      const out = join(sandbox.home, "results", "locked.json");
      const r = runPerf(
        ["--corpus", "typical", "--layouts", "split", "--out", out],
        {},
        { FAKE_SCREEN_LOCKED: "Yes" },
      );
      expect(r.code).toBe(2);
      expect(r.stderr).toContain("the screen is locked");
      expect(r.stderr).not.toContain("warm-up");
      expect(existsSync(out)).toBe(false);
      expect(existsSync(appLaunches)).toBe(false);
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
    "app scenarios run the app with POLYGLOSS_TEST=1 and the corpus entry",
    () => {
      writeFileSync(appLaunches, "");
      const out = join(sandbox.home, "results", "app.json");
      const r = runPerf(
        [
          "--corpus",
          "linux",
          "--layouts",
          "split",
          "--scenarios",
          "app-open",
          "--check-budgets",
          "--out",
          out,
        ],
        { APP_FIRST_PAINT: "640" },
      );
      expect({ code: r.code, stderr: r.stderr }).toMatchObject({ code: 0 });
      const results = JSON.parse(readFileSync(out, "utf8")) as Results;
      expect(results.rows[0]!.metrics.app_first_paint_ms).toBe(640);
      const run = results.runs.find((x) => x.scenario === "app-open")!;
      expect(run.result?.info).toMatchObject({
        polygloss_test: "1",
        repo: join(corporaRoot, "linux"),
        base: "v6.10",
        head: "v6.11",
        mode: "direct",
      });
      // A launch, one unmeasured window (the app's own shader cache), then
      // the measured run; the app is told the scenario by its own name.
      const lines = readFileSync(appLaunches, "utf8").trim().split("\n");
      expect(lines[0]).toBe("--version");
      expect(
        lines.slice(1).map((l) => l.split(" ").slice(0, 6).join(" ")),
      ).toEqual([
        "--perf-scenario open --corpus linux --layout split",
        "--perf-scenario open --corpus linux --layout split",
      ]);
      expect(results.warmup).toBeTruthy();
      expect(r.stdout).toContain("640 ✓");
      // Over budget (Linux < 2,000 ms) with --check-budgets fails.
      const slow = runPerf(
        [
          "--corpus",
          "linux",
          "--layouts",
          "split",
          "--scenarios",
          "app-open",
          "--check-budgets",
          "--out",
          join(sandbox.home, "results", "app-slow.json"),
        ],
        { APP_FIRST_PAINT: "2100" },
      );
      expect(slow.code).toBe(1);
      expect(slow.stderr).toContain("app_first_paint_ms");
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
