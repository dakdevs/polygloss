// The perf matrix (plan T2.9, design §12.1): runs `polygloss-perf` over the
// §12.2 corpora in both layouts, samples each run's RSS, prints a table,
// writes benches/results/<date>-<git-sha>.json and checks budgets and the
// baseline.
//
//   bun benches/run-perf.ts [--corpus all|<name>[,<name>…]] [--layouts split,unified]
//     [--scenarios <name>[,…]] [--check-budgets] [--compare-baseline]
//     [--write-baseline] [--baseline <path>] [--repeat <n>] [--bin <path>]
//     [--app-bin <path>] [--build] [--out <path>] [--dry-run]
//
// Every scenario runs in its own process (first paint is measured from
// process start): `polygloss-perf` for the viewport scenarios, the app itself
// (`Polygloss --perf-scenario <name>`, with POLYGLOSS_TEST=1, OQ-P4; `--app-bin`,
// default target/perf/Polygloss) for the app's (`app-open`: the app's own
// first paint, T3.1), each with a fresh sandboxed HOME, data, config and
// cache dir and an empty git config, on the corpora its metrics are budgeted
// for (budgets.json). Corpus paths and revisions come from
// benches/corpora/lib.ts `corpusEntry`, passed to the binary as flags.
// Each binary is launched once (`--version`) before the matrix, so macOS's
// first-launch assessment of a fresh build is not measured, and then runs one
// unmeasured `open` window (its runner's first planned corpus and layout), so
// a new binary's first-window Metal shader compilation (OQ-P12) is not
// either; those warm-ups' first paints are reported (`warmups` in the
// results; `warmup` is the first). `peak_rss_mb` is the largest RSS
// (`ps -o rss= -p`, every 100 ms) of any run of a corpus and layout.
// `--repeat n` runs everything n times; each metric is then the p95 (nearest
// rank) of its n per-run values. A disabled app scenario reports null.
//
// Exit codes: 1 when a run fails, with --check-budgets when a budget is
// missed, with --compare-baseline when a metric regressed by more than 10%
// against this machine's baseline entry (provisional), and when
// --write-baseline refuses; 2 for usage errors, a missing corpus or a missing
// harness binary.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { makeSandbox } from "../tests/support/sandbox";
import {
  CorpusError,
  type CorpusEntry,
  type CorpusName,
  corpusCommands,
  corpusEntry,
  corpusNames,
  hasCommits,
  isCorpusName,
} from "./corpora/lib";

const benchesDir = import.meta.dir;
const repoRoot = resolve(benchesDir, "..");

export const layoutNames = ["split", "unified"] as const;
export type LayoutName = (typeof layoutNames)[number];

export const metricNames = [
  "first_paint_ms",
  "scroll_p95_ms",
  "highlight_ms",
  "comment_repaint_ms",
  "watcher_banner_ms",
  "app_first_paint_ms",
  "peak_rss_mb",
] as const;
export type MetricName = (typeof metricNames)[number];

/** budgets.json: each metric's budget per corpus (design §12.1). */
export type Budgets = {
  metrics: Record<
    MetricName,
    { label: string; unit: string; budget: Partial<Record<CorpusName, number>> }
  >;
};

/** A regression is a value more than this fraction above the baseline. */
export const REGRESSION_TOLERANCE = 0.1;

/** Longest a single harness run may take before it is killed. */
const RUN_TIMEOUT_MS = 15 * 60 * 1000;

export function loadBudgets(
  path: string = join(benchesDir, "budgets.json"),
): Budgets {
  const budgets = JSON.parse(readFileSync(path, "utf8")) as Budgets;
  for (const name of metricNames)
    if (!budgets.metrics?.[name]) throw new Error(`${path} has no ${name}`);
  return budgets;
}

// ---- statistics ----

/**
 * The nearest-rank percentile: the smallest value with at least `p` percent
 * of the sample at or below it (the same definition as polygloss-perf's
 * metrics.rs). `null` for an empty sample.
 */
export function percentile(values: number[], p: number): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.max(1, Math.ceil((p / 100) * sorted.length));
  return sorted[Math.min(rank, sorted.length) - 1]!;
}

/** One metric over repeated runs: the p95 of the measured (non-null) values. */
export function aggregate(values: (number | null)[]): number | null {
  return percentile(
    values.filter((v): v is number => v !== null),
    95,
  );
}

// ---- rows, budgets, baseline ----

/** Every metric of one corpus in one layout (`null`: not measured). */
export type Row = {
  corpus: CorpusName;
  layout: LayoutName;
  metrics: Record<string, number | null>;
};

export type BudgetCheck = {
  corpus: CorpusName;
  layout: LayoutName;
  metric: MetricName;
  value: number | null;
  budget: number;
  /** `missing`: the metric was reported as null (not measured yet). */
  status: "pass" | "miss" | "missing";
};

/**
 * Checks every budgeted metric a row has (metrics a run did not produce are
 * skipped). A value passes only when strictly below its budget.
 */
export function checkBudgets(rows: Row[], budgets: Budgets): BudgetCheck[] {
  const checks: BudgetCheck[] = [];
  for (const row of rows)
    for (const metric of metricNames) {
      const budget = budgets.metrics[metric].budget[row.corpus];
      const value = row.metrics[metric];
      if (budget === undefined || value === undefined) continue;
      const status =
        value === null ? "missing" : value < budget ? "pass" : "miss";
      checks.push({
        corpus: row.corpus,
        layout: row.layout,
        metric,
        value,
        budget,
        status,
      });
    }
  return checks;
}

/** No budget was missed (null metrics do not count as misses). */
export function budgetsPassed(checks: BudgetCheck[]): boolean {
  return checks.every((c) => c.status !== "miss");
}

export type Machine = { cpu: string; macos: string };
export type BaselineEntry = {
  machine: Machine;
  git_sha: string;
  date: string;
  rows: Row[];
};
export type Baseline = { $comment?: string; entries: BaselineEntry[] };
export type Regression = {
  corpus: CorpusName;
  layout: LayoutName;
  metric: MetricName;
  baseline: number;
  value: number;
};

/** The baseline entry recorded on a machine with this CPU, if any. */
export function findBaseline(
  baseline: Baseline,
  machine: Machine,
): BaselineEntry | null {
  return baseline.entries.find((e) => e.machine.cpu === machine.cpu) ?? null;
}

/** `baseline` with `entry` replacing the entry of the same CPU (or added). */
export function withBaseline(
  baseline: Baseline,
  entry: BaselineEntry,
): Baseline {
  const i = baseline.entries.findIndex(
    (e) => e.machine.cpu === entry.machine.cpu,
  );
  const entries = [...baseline.entries];
  if (i >= 0) entries[i] = entry;
  else entries.push(entry);
  return { ...baseline, entries };
}

/**
 * Metrics budgeted for a row's corpus that are more than `tolerance` above
 * the baseline entry's value for the same corpus and layout. Metrics either
 * side did not measure cannot regress.
 */
export function compareBaseline(
  rows: Row[],
  entry: BaselineEntry,
  budgets: Budgets,
  tolerance: number = REGRESSION_TOLERANCE,
): Regression[] {
  const regressions: Regression[] = [];
  for (const row of rows) {
    const base = entry.rows.find(
      (b) => b.corpus === row.corpus && b.layout === row.layout,
    );
    if (!base) continue;
    for (const metric of metricNames) {
      if (budgets.metrics[metric].budget[row.corpus] === undefined) continue;
      const value = row.metrics[metric];
      const before = base.metrics[metric];
      if (typeof value !== "number" || typeof before !== "number") continue;
      if (value > before * (1 + tolerance))
        regressions.push({
          corpus: row.corpus,
          layout: row.layout,
          metric,
          baseline: before,
          value,
        });
    }
  }
  return regressions;
}

// ---- scenarios and the plan ----

export type Scenario = {
  name: string;
  /** `perf`: polygloss-perf; `app`: `Polygloss --perf-scenario` (OQ-P4). */
  runner: "perf" | "app";
  /** The name the binary knows the scenario by (default `name`). */
  binScenario?: string;
  /** The budgeted metrics it measures; it runs where they have budgets. */
  metrics: MetricName[];
  enabled: boolean;
  note?: string;
};

export const scenarios: Scenario[] = [
  { name: "open", runner: "perf", metrics: ["first_paint_ms"], enabled: true },
  { name: "scroll", runner: "perf", metrics: ["scroll_p95_ms"], enabled: true },
  {
    name: "highlight",
    runner: "perf",
    metrics: ["highlight_ms"],
    enabled: true,
  },
  // T3.1: first paint through the app's real startup.
  {
    name: "app-open",
    runner: "app",
    binScenario: "open",
    metrics: ["app_first_paint_ms"],
    enabled: true,
  },
  // T3.11: one save in a live working tree of the corpus → the banner.
  {
    name: "watcher-banner",
    runner: "app",
    metrics: ["watcher_banner_ms"],
    enabled: true,
  },
  // T3.10: a draft saved and a thread resolved through the app → the frame
  // that shows it (real thread blocks). It replaces M2's `blocks` scenario
  // of polygloss-perf (still there to run by hand, unbudgeted).
  {
    name: "comment-roundtrip",
    runner: "app",
    metrics: ["comment_repaint_ms"],
    enabled: true,
  },
];

export type PlannedRun = {
  scenario: string;
  runner: Scenario["runner"];
  corpus: CorpusName;
  layout: LayoutName;
  metrics: MetricName[];
  enabled: boolean;
};

/**
 * Every scenario (optionally only those named) on each requested corpus one
 * of its metrics has a budget for, in each layout: scenario, then corpus, then
 * layout order.
 */
export function planRuns(opts: {
  corpora: CorpusName[];
  layouts: LayoutName[];
  scenarios?: string[];
  budgets: Budgets;
}): PlannedRun[] {
  const runs: PlannedRun[] = [];
  for (const s of scenarios) {
    if (opts.scenarios && !opts.scenarios.includes(s.name)) continue;
    for (const corpus of corpusNames) {
      if (!opts.corpora.includes(corpus)) continue;
      const budgeted = s.metrics.some(
        (m) => opts.budgets.metrics[m].budget[corpus] !== undefined,
      );
      if (!budgeted) continue;
      for (const layout of layoutNames) {
        if (!opts.layouts.includes(layout)) continue;
        runs.push({
          scenario: s.name,
          runner: s.runner,
          corpus,
          layout,
          metrics: s.metrics,
          enabled: s.enabled,
        });
      }
    }
  }
  return runs;
}

/** The scenario name `run`'s binary knows (`app-open` is the app's `open`). */
export function binScenario(scenario: string): string {
  return scenarios.find((s) => s.name === scenario)?.binScenario ?? scenario;
}

/**
 * The unmeasured window run before the matrix for one runner: its `open`
 * (`open` for `polygloss-perf`, `app-open` for the app) on the first enabled
 * run of that runner's corpus and layout (`null` when it runs nothing).
 * gpui-kit forces runtime shaders (OQ-P12): GPUI compiles its Metal shaders
 * when it opens a window, and Metal caches the result. The first windowed
 * launch of a new binary can miss that cache (seen right after a rebuild and
 * for a copy at a new path); its window then opens ≈ 150–200 ms later than
 * on every later launch.
 */
export function warmupRun(
  plan: PlannedRun[],
  runner: Scenario["runner"] = "perf",
): PlannedRun | null {
  const first = plan.find((p) => p.enabled && p.runner === runner);
  if (!first) return null;
  const open =
    runner === "perf"
      ? { scenario: "open", metrics: ["first_paint_ms" as MetricName] }
      : { scenario: "app-open", metrics: ["app_first_paint_ms" as MetricName] };
  return {
    ...open,
    runner,
    corpus: first.corpus,
    layout: first.layout,
    enabled: true,
  };
}

// ---- table ----

function fmt(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

/**
 * One line per corpus and layout, one column per budgeted metric: the value
 * with ✓ or ✗ where it has a budget, `n/a` when not measured, `—` when not
 * run. Frame-to-frame interval p95 / max sit next to the scroll p95.
 */
export function formatTable(rows: Row[], checks: BudgetCheck[]): string {
  const header: string[] = ["corpus", "layout"];
  for (const m of metricNames) {
    header.push(m);
    if (m === "scroll_p95_ms") header.push("interval p95 / max");
  }
  const lines = [header];
  for (const row of rows) {
    const cells: string[] = [row.corpus, row.layout];
    for (const m of metricNames) {
      const value = row.metrics[m];
      const check = checks.find(
        (c) =>
          c.corpus === row.corpus && c.layout === row.layout && c.metric === m,
      );
      if (value === undefined) cells.push("—");
      else if (value === null) cells.push("n/a");
      else if (check)
        cells.push(`${fmt(value)} ${check.status === "pass" ? "✓" : "✗"}`);
      else cells.push(fmt(value));
      if (m === "scroll_p95_ms") {
        const p95 = row.metrics.frame_interval_p95_ms;
        const max = row.metrics.frame_interval_max_ms;
        cells.push(
          typeof p95 === "number" && typeof max === "number"
            ? `${fmt(p95)} / ${fmt(max)}`
            : "—",
        );
      }
    }
    lines.push(cells);
  }
  const widths = header.map((_, i) =>
    Math.max(...lines.map((l) => [...(l[i] ?? "")].length)),
  );
  return lines
    .map((l) =>
      l
        .map((cell, i) => cell + " ".repeat(widths[i]! - [...cell].length))
        .join("  ")
        .trimEnd(),
    )
    .join("\n");
}

// ---- running ----

/** What one harness process printed (plan T2.9 result shape). */
export type ScenarioResult = {
  scenario: string;
  corpus: string;
  layout: string;
  metrics: Record<string, number | null>;
  samples?: Record<string, number[]>;
  info?: Record<string, unknown>;
};

type RunRecord = {
  scenario: string;
  corpus: CorpusName;
  layout: LayoutName;
  repeat: number;
  argv: string[] | null;
  wall_ms: number | null;
  peak_rss_mb: number | null;
  result: ScenarioResult | null;
  error: string | null;
  note?: string;
};

/** The binaries of the two runners. */
export type Bins = { perf: string; app: string };

/**
 * The command for one run over `entry`: `polygloss-perf --corpus … --scenario
 * <name>`, or the app's `Polygloss --perf-scenario <name> --corpus …` (run
 * with POLYGLOSS_TEST=1, see `runEnv`).
 */
export function harnessArgs(
  bins: Bins,
  run: PlannedRun,
  entry: CorpusEntry,
): string[] {
  const head =
    run.runner === "app"
      ? [bins.app, "--perf-scenario", binScenario(run.scenario)]
      : [bins.perf];
  return [
    ...head,
    "--corpus",
    run.corpus,
    "--layout",
    run.layout,
    ...(run.runner === "perf" ? ["--scenario", run.scenario] : []),
    "--json",
    "--repo",
    entry.repo,
    "--base",
    entry.base,
    "--head",
    entry.head,
    ...(entry.mode === "direct" ? ["--direct"] : []),
  ];
}

/** Checks that `value` is a result for `run` and returns it. */
function parseResult(stdout: string, run: PlannedRun): ScenarioResult {
  let value: unknown;
  try {
    value = JSON.parse(stdout);
  } catch {
    throw new Error(`printed no JSON result: ${JSON.stringify(stdout)}`);
  }
  const r = value as ScenarioResult;
  if (
    typeof r !== "object" ||
    r === null ||
    r.scenario !== binScenario(run.scenario) ||
    r.corpus !== run.corpus ||
    r.layout !== run.layout
  )
    throw new Error(`printed a result for another run: ${stdout.trim()}`);
  if (typeof r.metrics !== "object" || r.metrics === null)
    throw new Error("result has no metrics");
  for (const [k, v] of Object.entries(r.metrics))
    if (v !== null && typeof v !== "number")
      throw new Error(`metric ${k} is not a number or null`);
  for (const m of run.metrics)
    if (!(m in r.metrics)) throw new Error(`result has no ${m}`);
  return r;
}

/** Resident set size of `pid` in KiB, or null once it has exited. */
async function rssKib(pid: number): Promise<number | null> {
  const ps = Bun.spawn(["ps", "-o", "rss=", "-p", String(pid)], {
    stdout: "pipe",
    stderr: "ignore",
  });
  const out = await new Response(ps.stdout).text();
  await ps.exited;
  const kib = Number.parseInt(out.trim(), 10);
  // An exited, not yet reaped process reports 0.
  return Number.isFinite(kib) && kib > 0 ? kib : null;
}

/** Extra environment of a run: the app's scenarios are test-only (OQ-P4). */
export function runEnv(run: PlannedRun): Record<string, string> {
  return run.runner === "app" ? { POLYGLOSS_TEST: "1" } : {};
}

/** Runs one harness process in a fresh sandbox, sampling its RSS. */
async function runHarness(
  argv: string[],
  env: Record<string, string>,
): Promise<{
  code: number;
  stdout: string;
  peakKib: number | null;
  ms: number;
  timedOut: boolean;
}> {
  const sandbox = makeSandbox();
  const started = performance.now();
  try {
    const child = Bun.spawn(argv, {
      env: { ...sandbox.env, ...env },
      stdout: "pipe",
      stderr: "inherit",
    });
    let peak: number | null = null;
    let running = true;
    const sampler = (async () => {
      while (running) {
        const kib = await rssKib(child.pid);
        if (kib !== null) peak = Math.max(peak ?? 0, kib);
        await Bun.sleep(100);
      }
    })();
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, RUN_TIMEOUT_MS);
    const stdout = await new Response(child.stdout).text();
    const code = await child.exited;
    running = false;
    clearTimeout(timer);
    await sampler;
    const ms = performance.now() - started;
    return { code, stdout, peakKib: peak, ms, timedOut };
  } finally {
    sandbox.cleanup();
  }
}

function newRecord(p: PlannedRun, repeat: number): RunRecord {
  return {
    scenario: p.scenario,
    corpus: p.corpus,
    layout: p.layout,
    repeat,
    argv: null,
    wall_ms: null,
    peak_rss_mb: null,
    result: null,
    error: null,
  };
}

/** Runs `argv` for `p` and fills in `record` (its result or its error). */
async function measure(
  record: RunRecord,
  p: PlannedRun,
  argv: string[],
): Promise<void> {
  record.argv = argv;
  const out = await runHarness(argv, runEnv(p));
  record.wall_ms = Math.round(out.ms);
  record.peak_rss_mb =
    out.peakKib === null ? null : Math.round((out.peakKib / 1024) * 10) / 10;
  try {
    if (out.timedOut)
      throw new Error(`killed after ${RUN_TIMEOUT_MS / 60_000} min`);
    if (out.code !== 0) throw new Error(`exited with code ${out.code}`);
    record.result = parseResult(out.stdout, p);
  } catch (e) {
    record.error = (e as Error).message;
  }
}

/** Rows per corpus and layout (corpus, then layout order) from every run. */
function buildRows(plan: PlannedRun[], records: RunRecord[]): Row[] {
  const rows: Row[] = [];
  const produced = new Set(
    plan.filter((p) => p.enabled).flatMap((p) => p.metrics),
  );
  for (const p of plan) {
    let row = rows.find((r) => r.corpus === p.corpus && r.layout === p.layout);
    if (!row) {
      row = { corpus: p.corpus, layout: p.layout, metrics: {} };
      rows.push(row);
    }
  }
  rows.sort(
    (a, b) =>
      corpusNames.indexOf(a.corpus) - corpusNames.indexOf(b.corpus) ||
      layoutNames.indexOf(a.layout) - layoutNames.indexOf(b.layout),
  );
  for (const row of rows) {
    const mine = records.filter(
      (r) => r.corpus === row.corpus && r.layout === row.layout,
    );
    const values = new Map<string, (number | null)[]>();
    for (const r of mine)
      for (const [k, v] of Object.entries(r.result?.metrics ?? {}))
        values.set(k, [...(values.get(k) ?? []), v]);
    for (const [k, vs] of values) row.metrics[k] = aggregate(vs);
    // Metrics of scenarios that do not run yet are reported as null.
    for (const p of plan)
      if (p.corpus === row.corpus && p.layout === row.layout && !p.enabled)
        for (const m of p.metrics)
          if (!produced.has(m) && !(m in row.metrics)) row.metrics[m] = null;
    const rss = mine
      .map((r) => r.peak_rss_mb)
      .filter((v): v is number => v !== null);
    if (rss.length > 0) row.metrics.peak_rss_mb = Math.max(...rss);
  }
  return rows;
}

function command(argv: string[]): string {
  const r = Bun.spawnSync(argv, { cwd: repoRoot, stderr: "ignore" });
  return r.exitCode === 0 ? r.stdout.toString().trim() : "";
}

/** CPU brand and macOS version (recorded in results and baselines). */
export function currentMachine(): Machine {
  return {
    cpu: command(["sysctl", "-n", "machdep.cpu.brand_string"]) || "unknown",
    macos: command(["sw_vers", "-productVersion"]) || "unknown",
  };
}

function gitState(): { sha: string; dirty: boolean } {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env))
    if (v !== undefined && !k.startsWith("GIT_")) env[k] = v;
  const git = (args: string[]) =>
    Bun.spawnSync(["git", "-C", repoRoot, ...args], { env, stderr: "ignore" });
  const head = git(["rev-parse", "--short=12", "HEAD"]);
  const status = git(["status", "--porcelain"]);
  return {
    sha: head.exitCode === 0 ? head.stdout.toString().trim() : "unknown",
    dirty: status.exitCode === 0 && status.stdout.toString().trim() !== "",
  };
}

function list(value: string, what: string, allowed: readonly string[]) {
  const items = value.split(",").filter((s) => s !== "");
  if (items.length === 0) throw new CorpusError(`no ${what} given`);
  for (const item of items)
    if (!allowed.includes(item))
      throw new CorpusError(
        `unknown ${what.replace(/s$/, "")} ${JSON.stringify(item)} (expected ${allowed.join(", ")})`,
      );
  return items;
}

const usage =
  "bun benches/run-perf.ts [--corpus all|<name>[,…]] [--layouts split,unified] " +
  "[--scenarios <name>[,…]] [--check-budgets] [--compare-baseline] [--write-baseline] " +
  "[--baseline <path>] [--repeat <n>] [--bin <path>] [--app-bin <path>] [--build] [--out <path>] [--dry-run]";

async function main(argv: string[]): Promise<number> {
  const { values } = parseArgs({
    args: argv,
    options: {
      corpus: { type: "string", default: "all" },
      layouts: { type: "string", default: "split,unified" },
      scenarios: { type: "string" },
      "check-budgets": { type: "boolean" },
      "compare-baseline": { type: "boolean" },
      "write-baseline": { type: "boolean" },
      baseline: { type: "string", default: join(benchesDir, "baseline.json") },
      repeat: { type: "string", default: "1" },
      bin: { type: "string" },
      "app-bin": { type: "string" },
      build: { type: "boolean" },
      out: { type: "string" },
      "dry-run": { type: "boolean" },
    },
    strict: true,
  });
  const corpora = (
    values.corpus === "all"
      ? [...corpusNames]
      : list(values.corpus, "corpora", corpusNames)
  ).filter(isCorpusName);
  const layouts = list(values.layouts, "layouts", layoutNames) as LayoutName[];
  const only =
    values.scenarios === undefined
      ? undefined
      : list(
          values.scenarios,
          "scenarios",
          scenarios.map((s) => s.name),
        );
  const repeat = Number(values.repeat);
  if (!Number.isInteger(repeat) || repeat < 1)
    throw new CorpusError(`--repeat must be a positive integer`);
  const budgets = loadBudgets();
  const plan = planRuns({ corpora, layouts, scenarios: only, budgets });
  const targetDir = resolve(repoRoot, process.env.CARGO_TARGET_DIR || "target");
  const bins: Bins = {
    perf: resolve(values.bin ?? join(targetDir, "perf", "polygloss-perf")),
    app: resolve(values["app-bin"] ?? join(targetDir, "perf", "Polygloss")),
  };
  const runners = (["perf", "app"] as const).filter((r) =>
    plan.some((p) => p.enabled && p.runner === r),
  );
  const entries = new Map(plan.map((p) => [p.corpus, corpusEntry(p.corpus)]));

  const warms = runners
    .map((r) => warmupRun(plan, r))
    .filter((w): w is PlannedRun => w !== null);
  if (values["dry-run"]) {
    for (const warm of warms)
      process.stdout.write(
        `warm-up (not measured): ${warm.scenario} ${warm.corpus} ${warm.layout}: ${harnessArgs(bins, warm, entries.get(warm.corpus)!).join(" ")}\n`,
      );
    for (const p of plan) {
      const how = p.enabled
        ? harnessArgs(bins, p, entries.get(p.corpus)!).join(" ")
        : `not run (${scenarios.find((s) => s.name === p.scenario)?.note})`;
      process.stdout.write(`${p.scenario} ${p.corpus} ${p.layout}: ${how}\n`);
    }
    return 0;
  }

  let missing = 0;
  for (const e of entries.values())
    if (!hasCommits(e.repo, [e.base, e.head])) {
      missing++;
      process.stderr.write(
        `run-perf: the ${e.name} corpus is missing at ${e.repo}; run ${corpusCommands[e.name]}\n`,
      );
    }
  if (missing > 0) return 2;
  const packages = { perf: "polygloss-perf", app: "polygloss-app" } as const;
  if (values.build)
    // One package per build, so features are not unified across them.
    for (const runner of runners) {
      const build = ["scripts/cargo.sh", "build", "--profile", "perf"];
      const r = Bun.spawnSync([...build, "-p", packages[runner]], {
        cwd: repoRoot,
        stdout: "inherit",
        stderr: "inherit",
      });
      if (r.exitCode !== 0) return 1;
    }
  for (const runner of runners) {
    const bin = bins[runner];
    if (!existsSync(bin)) {
      process.stderr.write(
        `run-perf: no ${runner === "app" ? "app" : "harness"} at ${bin}; run scripts/cargo.sh build --profile perf -p ${packages[runner]} (or pass --build)\n`,
      );
      return 2;
    }
    // The first launch of a freshly built binary pays macOS's code
    // assessment (about 2 s); launch it once so that stays out of first
    // paint.
    const warmup = makeSandbox();
    try {
      const r = Bun.spawnSync([bin, "--version"], {
        env: warmup.env,
        stdout: "ignore",
        stderr: "pipe",
      });
      if (r.exitCode !== 0) {
        process.stderr.write(
          `run-perf: the harness at ${bin} does not start: ${r.stderr.toString().trim()}\n`,
        );
        return 2;
      }
    } finally {
      warmup.cleanup();
    }
  }
  // One unmeasured window per binary keeps a new binary's shader
  // compilation out of the matrix (see `warmupRun`; the app has its own
  // Metal cache, OQ-P12); its first paint is still reported.
  const warmupRecords: RunRecord[] = [];
  for (const warm of warms) {
    const record = newRecord(warm, 0);
    warmupRecords.push(record);
    process.stderr.write(
      `run-perf: warm-up (not measured): ${warm.scenario} ${warm.corpus} ${warm.layout}\n`,
    );
    await measure(
      record,
      warm,
      harnessArgs(bins, warm, entries.get(warm.corpus)!),
    );
    const fp = record.result?.metrics[warm.metrics[0]!];
    process.stderr.write(
      record.error === null
        ? `run-perf: warm-up first paint ${fp} ms (not counted: a new binary's first window can compile GPUI's shaders, OQ-P12)\n`
        : `run-perf: warm-up failed: ${record.error} (continuing)\n`,
    );
  }
  const warmupRecord = warmupRecords[0] ?? null;

  const machine = currentMachine();
  const git = gitState();
  const date = new Date().toISOString().slice(0, 10);
  const records: RunRecord[] = [];
  const total = plan.filter((p) => p.enabled).length * repeat;
  let n = 0;
  for (let i = 0; i < repeat; i++)
    for (const p of plan) {
      const record = newRecord(p, i);
      records.push(record);
      if (!p.enabled) {
        record.note = scenarios.find((s) => s.name === p.scenario)?.note;
        continue;
      }
      n++;
      process.stderr.write(
        `run-perf: [${n}/${total}] ${p.scenario} ${p.corpus} ${p.layout}\n`,
      );
      await measure(record, p, harnessArgs(bins, p, entries.get(p.corpus)!));
      if (record.error !== null)
        process.stderr.write(
          `run-perf: ${p.scenario} ${p.corpus} ${p.layout} failed: ${record.error}\n`,
        );
    }

  const rows = buildRows(plan, records);
  const checks = checkBudgets(rows, budgets);
  process.stdout.write(`${formatTable(rows, checks)}\n`);
  const failed = records.filter((r) => r.error !== null);
  let code = failed.length > 0 ? 1 : 0;

  if (values["check-budgets"])
    for (const c of checks.filter((c) => c.status === "miss")) {
      code = 1;
      process.stderr.write(
        `run-perf: budget missed: ${c.metric} ${c.corpus} ${c.layout} = ${c.value} (budget < ${c.budget})\n`,
      );
    }

  const baselinePath = resolve(values.baseline);
  const baseline = existsSync(baselinePath)
    ? (JSON.parse(readFileSync(baselinePath, "utf8")) as Baseline)
    : { entries: [] };
  let regressions: Regression[] | null = null;
  if (values["compare-baseline"]) {
    const entry = findBaseline(baseline, machine);
    if (!entry)
      process.stderr.write(
        `run-perf: no baseline for ${machine.cpu} in ${baselinePath}; nothing to compare\n`,
      );
    else {
      regressions = compareBaseline(rows, entry, budgets);
      for (const r of regressions) {
        code = 1;
        process.stderr.write(
          `run-perf: ${r.metric} ${r.corpus} ${r.layout} regressed: ${r.value} vs baseline ${r.baseline} (> ${REGRESSION_TOLERANCE * 100}%)\n`,
        );
      }
    }
  }

  const out =
    values.out ??
    join(
      benchesDir,
      "results",
      `${date}-${git.sha}${git.dirty ? "-dirty" : ""}.json`,
    );
  mkdirSync(dirname(resolve(out)), { recursive: true });
  writeFileSync(
    out,
    `${JSON.stringify(
      {
        date,
        git_sha: git.sha,
        dirty: git.dirty,
        machine,
        bin: bins.perf,
        app_bin: bins.app,
        repeat,
        rows,
        budgets: checks,
        regressions,
        warmup: warmupRecord,
        warmups: warmupRecords,
        runs: records,
      },
      null,
      2,
    )}\n`,
  );
  process.stderr.write(`run-perf: results in ${out}\n`);

  if (values["write-baseline"]) {
    const missed = values["check-budgets"] && !budgetsPassed(checks);
    if (failed.length > 0 || missed) {
      process.stderr.write(
        `run-perf: not writing a baseline from a run that ${failed.length > 0 ? "failed" : "missed a budget"}\n`,
      );
      return 1;
    }
    const next = withBaseline(baseline, {
      machine,
      git_sha: git.sha,
      date,
      rows,
    });
    writeFileSync(baselinePath, `${JSON.stringify(next, null, 2)}\n`);
    process.stderr.write(`run-perf: baseline for ${machine.cpu} written\n`);
  }
  return code;
}

if (import.meta.main) {
  try {
    process.exit(await main(process.argv.slice(2)));
  } catch (e) {
    const usageError =
      e instanceof CorpusError ||
      (e instanceof TypeError &&
        String((e as { code?: string }).code).startsWith("ERR_PARSE_ARGS"));
    if (!usageError) throw e;
    process.stderr.write(
      `run-perf: ${(e as Error).message}\nusage: ${usage}\n`,
    );
    process.exit(2);
  }
}
