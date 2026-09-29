// Hunk parity against the system git on a real range (plan T1.16, design §6.3).
//
//   bun scripts/git-parity.ts --repo <path> --range <a>..<b> [--min-rate 0.999]
//       [--algorithm myers|histogram] [--mismatches-dir <dir>]
//
// Runs `polygloss-cli debug parity` (hidden subcommand) on the two revisions,
// prints a summary table and exits 1 when the share of identical files is below
// --min-rate (default 0.999, the provisional §6.3 target). --algorithm changes
// only our side (git always runs Myers). --mismatches-dir writes every mismatch
// as `<n>.ours.diff` / `<n>.git.diff` plus `summary.json`, for CI artifacts.
//
// The CLI is `$POLYGLOSS_CLI_BIN` when set, else this checkout's debug build,
// built first through scripts/cargo.sh. Exit 2: usage error or tool failure.
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");

type Report = {
  files: number;
  identical: number;
  mismatches: Array<{ path: string; ours: string; git: string }>;
};

function fail(message: string): never {
  process.stderr.write(
    `git-parity: ${message}\n` +
      "usage: bun scripts/git-parity.ts --repo <path> --range <a>..<b> [--min-rate 0.999]\n" +
      "         [--algorithm myers|histogram] [--mismatches-dir <dir>]\n",
  );
  process.exit(2);
}

function parseArgs(argv: string[]): {
  repo: string;
  base: string;
  head: string;
  minRate: number;
  algorithm: "myers" | "histogram";
  mismatchesDir?: string;
} {
  const flags = [
    "--repo",
    "--range",
    "--min-rate",
    "--algorithm",
    "--mismatches-dir",
  ];
  const values: Record<string, string> = {};
  for (let i = 0; i < argv.length; i += 2) {
    const flag = argv[i] ?? "";
    const value = argv[i + 1];
    if (!flags.includes(flag) || value === undefined)
      fail(`unexpected argument ${JSON.stringify(flag)}`);
    values[flag] = value;
  }
  const repo = values["--repo"];
  if (!repo) fail("--repo is required");
  const range = values["--range"] ?? "";
  const parts = range.split("..");
  if (parts.length !== 2 || !parts[0] || !parts[1] || range.includes("..."))
    fail(`--range must be <base>..<head>, got ${JSON.stringify(range)}`);
  const minRate = Number(values["--min-rate"] ?? "0.999");
  if (!Number.isFinite(minRate) || minRate < 0 || minRate > 1)
    fail("--min-rate must be a number between 0 and 1");
  const algorithm = values["--algorithm"] ?? "myers";
  if (algorithm !== "myers" && algorithm !== "histogram")
    fail("--algorithm must be myers or histogram");
  return {
    repo,
    base: parts[0],
    head: parts[1],
    minRate,
    algorithm,
    mismatchesDir: values["--mismatches-dir"],
  };
}

function cli(): string {
  const override = process.env.POLYGLOSS_CLI_BIN;
  if (override) return resolve(override);
  const build = Bun.spawnSync(
    ["scripts/cargo.sh", "build", "-p", "polygloss-cli"],
    {
      cwd: repoRoot,
      stdout: "inherit",
      stderr: "inherit",
    },
  );
  if (build.exitCode !== 0)
    fail("building polygloss-cli failed (cargo's errors are above)");
  return resolve(
    repoRoot,
    process.env.CARGO_TARGET_DIR || "target",
    "debug",
    "polygloss-cli",
  );
}

function percent(rate: number): string {
  return `${(rate * 100).toFixed(3)}%`;
}

function writeMismatches(dir: string, report: Report): void {
  mkdirSync(dir, { recursive: true });
  const entries = report.mismatches.map((m, i) => {
    const n = String(i + 1).padStart(4, "0");
    const ours = `${n}.ours.diff`;
    const git = `${n}.git.diff`;
    writeFileSync(join(dir, ours), m.ours);
    writeFileSync(join(dir, git), m.git);
    return { path: m.path, ours, git };
  });
  writeFileSync(
    join(dir, "summary.json"),
    `${JSON.stringify({ files: report.files, identical: report.identical, mismatches: entries }, null, 2)}\n`,
  );
}

if (import.meta.main) {
  const args = parseArgs(process.argv.slice(2));
  const r = Bun.spawnSync(
    [
      cli(),
      "debug",
      "parity",
      "--repo",
      args.repo,
      "--base",
      args.base,
      "--head",
      args.head,
      "--algorithm",
      args.algorithm,
      "--json",
    ],
    { stdout: "pipe", stderr: "pipe" },
  );
  if (r.exitCode !== 0)
    fail(`polygloss-cli debug parity failed:\n${r.stderr.toString()}`);
  const report = JSON.parse(r.stdout.toString()) as Report;
  // No text pairs means nothing can differ.
  const rate = report.files === 0 ? 1 : report.identical / report.files;

  const rows: Array<[string, string]> = [
    ["repo", args.repo],
    ["range", `${args.base}..${args.head}`],
    ["algorithm", args.algorithm],
    ["files", String(report.files)],
    ["identical", String(report.identical)],
    ["mismatches", String(report.mismatches.length)],
    ["rate", `${percent(rate)} (min ${percent(args.minRate)})`],
  ];
  let out = rows.map(([k, v]) => `${k.padEnd(11)} ${v}`).join("\n") + "\n";
  for (const m of report.mismatches) out += `  mismatch  ${m.path}\n`;
  process.stdout.write(out);
  if (args.mismatchesDir) writeMismatches(args.mismatchesDir, report);

  if (rate < args.minRate) {
    process.stderr.write(
      `git-parity: ${percent(rate)} identical is below the minimum ${percent(args.minRate)}\n`,
    );
    process.exit(1);
  }
}
