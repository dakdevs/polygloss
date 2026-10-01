// Local dry run of the release workflow (plan T5.8):
//
//   bun scripts/release-dry-run.ts [--workflow .github/workflows/release.yml]
//     [--event workflow_dispatch|push] [--ref <git ref>] [--repository owner/name]
//     [--var NAME=value]… [--skip <step>]… [--only <step>] [--plan] [--json]
//
// Runs the workflow's steps on this machine in order, the way GitHub Actions
// would for one event: `if:` conditions are evaluated (a failed step skips
// the rest except `always()`/`failure()` steps), `${{ … }}` expressions are
// filled in (every secret and `github.token` empty, `vars.*` from --var,
// else empty), `$GITHUB_ENV` lines carry over to later steps, and each `run`
// goes through `bash --noprofile --norc -eo pipefail` with `RUNNER_TEMP` a
// fresh temp dir. The environment starts clean apart from PATH, HOME, USER,
// LOGNAME, SHELL, TMPDIR, LANG, TERM, CARGO_HOME and RUSTUP_HOME, so the
// caller's own APPLE_*/POLYGLOSS_* values never reach a step. Steps that `uses:`
// an action are not run (`action`), except actions/upload-artifact, whose
// path is checked against the files on disk (`if-no-files-found: error`
// fails when nothing matches). A step whose script calls `gh` or `git … push`
// is never run (`refuse-publish`): a local dry run publishes nothing.
//
// The release check before the first tag (plan T5.8, no GitHub remote yet):
// `bun scripts/release-dry-run.ts --event workflow_dispatch` (the workflow's
// dry-run path, unsigned without secrets). `--plan` prints each step's
// decision without running anything (`--json` for a machine-readable plan).
// Steps run in the current directory (the checkout).
//
// Exit codes: 0 every run step passed, 1 a step failed, 2 usage.
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";

type Ctx = {
  event: string;
  ref: string;
  repository: string;
  runnerTemp: string;
  vars: Record<string, string>;
};

type Step = {
  name?: string;
  uses?: string;
  run?: string;
  if?: string;
  env?: Record<string, string>;
  with?: Record<string, unknown>;
};

type Workflow = {
  env?: Record<string, string>;
  jobs?: Record<string, { env?: Record<string, string>; steps?: Step[] }>;
};

export type Decision =
  | "run"
  | "ok"
  | "failed"
  | "action"
  | "artifact-check"
  | "skip-if"
  | "skipped"
  | "refuse-publish";

const EVENTS = ["workflow_dispatch", "push"] as const;

const INHERITED = [
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "SHELL",
  "TMPDIR",
  "LANG",
  "TERM",
  "CARGO_HOME",
  "RUSTUP_HOME",
];

const refName = (ref: string) => ref.replace(/^refs\/(heads|tags)\//, "");

/** The value of a context path such as `github.ref` or `secrets.X`. */
function contextValue(path: string, ctx: Ctx): string {
  if (/^secrets\.\w+$/.test(path)) return "";
  const v = path.match(/^vars\.(\w+)$/);
  if (v) return ctx.vars[v[1]!] ?? "";
  switch (path) {
    case "github.token":
      return "";
    case "github.ref":
      return ctx.ref;
    case "github.ref_name":
      return refName(ctx.ref);
    case "github.event_name":
      return ctx.event;
    case "github.repository":
      return ctx.repository;
    case "runner.temp":
      return ctx.runnerTemp;
  }
  throw new Error(`unsupported expression \`${path}\``);
}

/** A literal ('…') or a context path. */
function operand(text: string, ctx: Ctx): string {
  const t = text.trim();
  const lit = t.match(/^'((?:[^']|'')*)'$/);
  if (lit) return lit[1]!.replaceAll("''", "'");
  return contextValue(t, ctx);
}

/** Fills in every `${{ … }}` in `text`; throws on an expression it does not support. */
export function expandExpressions(text: string, ctx: Ctx): string {
  return text.replace(/\$\{\{\s*(.*?)\s*\}\}/g, (_, expr: string) =>
    operand(expr, ctx),
  );
}

/** A status-free condition: `startsWith(a, b)`, `a == b`, `a != b`. */
function evaluateExpr(expr: string, ctx: Ctx): boolean {
  const sw = expr.match(/^startsWith\(\s*(.+?)\s*,\s*(.+?)\s*\)$/);
  if (sw) return operand(sw[1]!, ctx).startsWith(operand(sw[2]!, ctx));
  const cmp = expr.match(/^(.+?)\s*(==|!=)\s*(.+)$/);
  if (cmp) {
    const eq = operand(cmp[1]!, ctx) === operand(cmp[3]!, ctx);
    return cmp[2] === "==" ? eq : !eq;
  }
  throw new Error(`unsupported condition \`${expr}\``);
}

/**
 * Whether a step with condition `cond` runs, given whether an earlier step
 * failed. Without a status function a condition implies `success()`.
 */
export function evaluateCondition(
  cond: string | undefined,
  ctx: Ctx,
  failed: boolean,
): boolean {
  if (cond === undefined) return !failed;
  const expr = cond
    .trim()
    .replace(/^\$\{\{\s*(.*?)\s*\}\}$/s, "$1")
    .trim();
  switch (expr) {
    case "always()":
      return true;
    case "failure()":
      return failed;
    case "success()":
      return !failed;
    case "cancelled()":
      return false;
  }
  if (/&&|\|\|/.test(expr))
    throw new Error(`unsupported condition \`${expr}\``);
  // Evaluated even after a failure, so an unsupported condition always fails loudly.
  const value = evaluateExpr(expr, ctx);
  return !failed && value;
}

export function stepName(step: Step): string {
  return step.name ?? step.uses ?? (step.run ?? "").trim().split("\n")[0]!;
}

/** A script that calls `gh` or `git … push`: never run locally. */
function publishes(run: string): boolean {
  return run
    .split("\n")
    .some(
      (line) => /(^|[\s;&|(])gh\s/.test(line) || /\bgit\b.*\spush\b/.test(line),
    );
}

function stepsOf(
  wf: Workflow,
): { step: Step; jobEnv: Record<string, string> }[] {
  return Object.values(wf.jobs ?? {}).flatMap((job) =>
    (job.steps ?? []).map((step) => ({ step, jobEnv: job.env ?? {} })),
  );
}

class UsageError extends Error {}

function readGithubEnv(path: string): Record<string, string> {
  const out: Record<string, string> = {};
  if (!existsSync(path)) return out;
  for (const line of readFileSync(path, "utf8").split("\n")) {
    if (line === "") continue;
    if (line.includes("<<"))
      throw new Error(
        `multi-line GITHUB_ENV values are not supported: ${line}`,
      );
    const eq = line.indexOf("=");
    if (eq > 0) out[line.slice(0, eq)] = line.slice(eq + 1);
  }
  return out;
}

function expandEnv(
  env: Record<string, unknown> | undefined,
  ctx: Ctx,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(env ?? {}))
    out[k] = expandExpressions(String(v), ctx);
  return out;
}

/** actions/upload-artifact: the files its `path` matches, relative to `cwd`. */
function artifactFiles(step: Step, ctx: Ctx, cwd: string): string[] {
  const paths = expandExpressions(String(step.with?.path ?? ""), ctx)
    .split("\n")
    .map((p) => p.trim())
    .filter(Boolean);
  return paths
    .flatMap((p) => [...new Bun.Glob(p).scanSync({ cwd, onlyFiles: true })])
    .sort();
}

const isArtifactUpload = (step: Step) =>
  /^actions\/upload-artifact@/.test(step.uses ?? "");

async function main(argv: string[]): Promise<number> {
  let opts: {
    workflow: string;
    event: string;
    ref: string;
    repository: string;
    vars: Record<string, string>;
    skip: string[];
    only?: string;
    plan: boolean;
    json: boolean;
  };
  let wf: Workflow;
  try {
    const { values } = parseArgs({
      args: argv,
      options: {
        workflow: { type: "string", default: ".github/workflows/release.yml" },
        event: { type: "string", default: "workflow_dispatch" },
        ref: { type: "string" },
        repository: { type: "string", default: "owner/polygloss" },
        var: { type: "string", multiple: true, default: [] },
        skip: { type: "string", multiple: true, default: [] },
        only: { type: "string" },
        plan: { type: "boolean", default: false },
        json: { type: "boolean", default: false },
      },
    });
    if (!(EVENTS as readonly string[]).includes(values.event))
      throw new UsageError(`--event must be one of ${EVENTS.join(", ")}`);
    const ref =
      values.ref ?? (values.event === "push" ? undefined : "refs/heads/main");
    if (!ref || !ref.startsWith("refs/"))
      throw new UsageError("--ref refs/… is required for push");
    const vars: Record<string, string> = {};
    for (const kv of values.var) {
      const eq = kv.indexOf("=");
      if (eq <= 0) throw new UsageError(`--var wants NAME=value, not ${kv}`);
      vars[kv.slice(0, eq)] = kv.slice(eq + 1);
    }
    if (!existsSync(values.workflow))
      throw new UsageError(`no workflow at ${values.workflow}`);
    wf = Bun.YAML.parse(readFileSync(values.workflow, "utf8")) as Workflow;
    const names = stepsOf(wf).map(({ step }) => stepName(step));
    for (const n of [...values.skip, ...(values.only ? [values.only] : [])])
      if (!names.includes(n))
        throw new UsageError(`no step named ${JSON.stringify(n)}`);
    opts = {
      workflow: values.workflow,
      event: values.event,
      ref,
      repository: values.repository,
      vars,
      skip: values.skip,
      only: values.only,
      plan: values.plan,
      json: values.json,
    };
  } catch (e) {
    console.error(
      `release-dry-run: ${(e as Error).message}\nusage: bun scripts/release-dry-run.ts [--workflow <path>] [--event workflow_dispatch|push] [--ref <ref>] [--var NAME=value]… [--skip <step>]… [--only <step>] [--plan] [--json]`,
    );
    return 2;
  }

  const cwd = process.cwd();
  const runnerTemp = mkdtempSync(join(tmpdir(), "release-dry-run-"));
  const ctx: Ctx = {
    event: opts.event,
    ref: opts.ref,
    repository: opts.repository,
    runnerTemp,
    vars: opts.vars,
  };
  const githubEnvFile = join(runnerTemp, "github-env");
  writeFileSync(githubEnvFile, "");
  const base: Record<string, string> = {};
  for (const k of INHERITED)
    if (process.env[k] !== undefined) base[k] = process.env[k]!;
  Object.assign(base, {
    GITHUB_REF: ctx.ref,
    GITHUB_REF_NAME: refName(ctx.ref),
    GITHUB_EVENT_NAME: ctx.event,
    GITHUB_REPOSITORY: ctx.repository,
    GITHUB_WORKSPACE: cwd,
    GITHUB_ENV: githubEnvFile,
    RUNNER_TEMP: runnerTemp,
  });

  const results: { name: string; decision: Decision }[] = [];
  let failed = false;
  try {
    const workflowEnv = expandEnv(wf.env, ctx);
    let i = 0;
    for (const { step, jobEnv } of stepsOf(wf)) {
      i++;
      const name = stepName(step);
      const record = (decision: Decision) => results.push({ name, decision });
      if (
        opts.skip.includes(name) ||
        (opts.only !== undefined && opts.only !== name)
      ) {
        record("skipped");
        continue;
      }
      if (!evaluateCondition(step.if, ctx, failed)) {
        record("skip-if");
        continue;
      }
      if (step.uses !== undefined) {
        if (!isArtifactUpload(step)) {
          record("action");
          continue;
        }
        if (opts.plan) {
          record("artifact-check");
          continue;
        }
        const files = artifactFiles(step, ctx, cwd);
        const required =
          String(step.with?.["if-no-files-found"] ?? "warn") === "error";
        if (files.length === 0 && required) {
          console.error(
            `release-dry-run: ${name}: no files match ${String(step.with?.path ?? "")}`,
          );
          failed = true;
          record("failed");
        } else {
          console.log(
            `release-dry-run: ${name}: would upload ${files.join(", ") || "nothing"}`,
          );
          record("ok");
        }
        continue;
      }
      const run = expandExpressions(step.run ?? "", ctx);
      if (publishes(run)) {
        record("refuse-publish");
        continue;
      }
      const env = {
        ...base,
        ...workflowEnv,
        ...expandEnv(jobEnv, ctx),
        ...readGithubEnv(githubEnvFile),
        ...expandEnv(step.env, ctx),
      };
      if (opts.plan) {
        record("run");
        continue;
      }
      console.log(`release-dry-run: ▶ ${name}`);
      const scriptFile = join(runnerTemp, `step-${i}.sh`);
      writeFileSync(scriptFile, run);
      const r = Bun.spawnSync(
        ["bash", "--noprofile", "--norc", "-eo", "pipefail", scriptFile],
        {
          cwd,
          env,
          stdin: "ignore",
          stdout: "inherit",
          stderr: "inherit",
        },
      );
      if (r.exitCode === 0) record("ok");
      else {
        failed = true;
        record("failed");
      }
    }
  } catch (e) {
    console.error(`release-dry-run: ${(e as Error).message}`);
    rmSync(runnerTemp, { recursive: true, force: true });
    return 2;
  }
  rmSync(runnerTemp, { recursive: true, force: true });

  if (opts.json) {
    console.log(
      JSON.stringify({ event: ctx.event, ref: ctx.ref, steps: results }),
    );
  } else {
    console.log(
      `release-dry-run: ${opts.workflow} on ${ctx.event} (${ctx.ref})`,
    );
    for (const r of results)
      console.log(`  ${r.decision.padEnd(15)} ${r.name}`);
  }
  return failed ? 1 : 0;
}

if (import.meta.main) process.exit(await main(process.argv.slice(2)));
