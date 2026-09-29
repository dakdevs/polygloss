// Shared plumbing for the perf corpora (plan T2.1, design §12.2): where each
// corpus lives, its manifest entry, and writing a generated corpus repo.
//
// Root: $POLYGLOSS_CORPORA, else `polygloss-corpora` next to the main checkout
// (the parent of `git rev-parse --git-common-dir`), so every worktree shares
// one set outside the repo. Generated corpora live in <root>/generated/<name>;
// the Linux corpus in $POLYGLOSS_LINUX_REPO, else <root>/linux.
// fetch-linux.sh resolves the Linux path the same way.
import {
  existsSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import {
  buildParityRepo,
  type ParityEntry,
} from "../../scripts/make-parity-repo";

export {
  makeRng,
  type ParityEntry,
  type Rng,
} from "../../scripts/make-parity-repo";

export const corpusNames = [
  "typical",
  "synthetic",
  "huge-file",
  "linux",
] as const;
export type CorpusName = (typeof corpusNames)[number];

/** How the perf harness opens a corpus: `compare <base> <head> [--direct]`. */
export type CorpusEntry = {
  name: CorpusName;
  repo: string;
  base: string;
  head: string;
  mode: "three-dot" | "direct";
};

type Env = Record<string, string | undefined>;

/** The command that creates each corpus (for error messages). */
export const corpusCommands: Record<CorpusName, string> = {
  typical: "bun benches/corpora/make-typical.ts",
  synthetic: "bun benches/corpora/make-synthetic.ts",
  "huge-file": "bun benches/corpora/make-huge-file.ts",
  linux: "benches/corpora/fetch-linux.sh",
};

/** Tags every generated corpus repo carries (`buildParityRepo` label). */
const label = "corpus";
const baseTag = `${label}-base`;
const headTag = `${label}-head`;

const checkout = realpathSync(resolve(import.meta.dir, "../.."));

// Variables that would point git at some other repository.
const redirecting = [
  "GIT_DIR",
  "GIT_WORK_TREE",
  "GIT_INDEX_FILE",
  "GIT_OBJECT_DIRECTORY",
  "GIT_COMMON_DIR",
  "GIT_ALTERNATE_OBJECT_DIRECTORIES",
  "GIT_NAMESPACE",
  "GIT_CEILING_DIRECTORIES",
  "GIT_CONFIG_PARAMETERS",
  "GIT_CONFIG_COUNT",
];

function gitEnv(extra: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env))
    if (v !== undefined && !redirecting.includes(k)) env[k] = v;
  return { ...env, LC_ALL: "C", GIT_OPTIONAL_LOCKS: "0", ...extra };
}

export function isCorpusName(name: string): name is CorpusName {
  return (corpusNames as readonly string[]).includes(name);
}

/** The checkout that owns the git common dir (this one unless a worktree). */
function mainCheckout(): string {
  // A source tarball has no .git: the checkout is its own main checkout. Inside
  // a git checkout a failing git is an error, never a silent second location.
  if (!existsSync(join(checkout, ".git"))) return checkout;
  const r = Bun.spawnSync(
    ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
    { cwd: checkout, env: gitEnv({}) },
  );
  if (r.exitCode !== 0)
    throw new Error(
      `git rev-parse --git-common-dir failed in ${checkout}: ${r.stderr.toString()}`,
    );
  return dirname(r.stdout.toString().replace(/\n$/, ""));
}

/** `$POLYGLOSS_CORPORA`, else `polygloss-corpora` next to the main checkout. */
export function corporaRoot(env: Env = process.env): string {
  const set = env.POLYGLOSS_CORPORA;
  if (set) return resolve(set);
  return join(dirname(mainCheckout()), "polygloss-corpora");
}

/** Where `make-<name>.ts` writes; `variant` separates e.g. scaled builds. */
export function generatedRepo(
  name: Exclude<CorpusName, "linux">,
  env: Env = process.env,
  variant = "",
): string {
  return join(corporaRoot(env), "generated", `${name}${variant}`);
}

/** The manifest entry for `name` (paths only; nothing is read or created). */
export function corpusEntry(
  name: CorpusName,
  env: Env = process.env,
): CorpusEntry {
  if (name === "linux") {
    const repo = env.POLYGLOSS_LINUX_REPO
      ? resolve(env.POLYGLOSS_LINUX_REPO)
      : join(corporaRoot(env), "linux");
    return { name, repo, base: "v6.10", head: "v6.11", mode: "direct" };
  }
  return {
    name,
    repo: generatedRepo(name, env),
    base: baseTag,
    head: headTag,
    mode: "three-dot",
  };
}

/** Every rev resolves to a commit in `repo` itself (never a parent repo). */
export function hasCommits(repo: string, revs: string[]): boolean {
  if (!existsSync(repo)) return false;
  const env = gitEnv({ GIT_CEILING_DIRECTORIES: dirname(resolve(repo)) });
  // `--verify` takes exactly one revision.
  return revs.every(
    (rev) =>
      Bun.spawnSync(
        [
          "git",
          "-C",
          repo,
          "rev-parse",
          "--verify",
          "--quiet",
          `${rev}^{commit}`,
        ],
        { env },
      ).exitCode === 0,
  );
}

/** A user-facing failure: the CLI prints the message and exits 2. */
export class CorpusError extends Error {}

/**
 * Writes a generated corpus at `repo`: a repo whose `corpus-base` commit holds
 * exactly `base` and whose child `corpus-head` (checked out on `main`) holds
 * exactly `head`, with fixed identity and dates, so the same input always
 * gives the same object ids. It is built next to `repo` and then swapped in,
 * replacing an earlier generated corpus; any other non-empty directory there
 * is left alone (CorpusError).
 */
export function writeCorpus(opts: {
  repo: string;
  base: ParityEntry[];
  head: ParityEntry[];
}): void {
  const out = resolve(opts.repo);
  const occupied = existsSync(out) && readdirSync(out).length > 0;
  if (occupied && !hasCommits(out, [baseTag, headTag]))
    throw new CorpusError(
      `${out} exists and is not a generated corpus; refusing to replace it`,
    );
  mkdirSync(dirname(out), { recursive: true });
  const partial = `${out}.partial-${process.pid}`;
  rmSync(partial, { recursive: true, force: true });
  try {
    buildParityRepo({ out: partial, base: opts.base, head: opts.head, label });
    rmSync(out, { recursive: true, force: true });
    renameSync(partial, out);
  } finally {
    rmSync(partial, { recursive: true, force: true });
  }
}

/** Prints an entry as one line of JSON (the generators' output). */
export function printEntry(entry: CorpusEntry): void {
  process.stdout.write(`${JSON.stringify(entry)}\n`);
}

/**
 * Runs a CLI body: a CorpusError (or a bad flag from `util.parseArgs`) prints
 * `<tool>: <message>` plus usage and exits 2.
 */
export function runCli(opts: {
  tool: string;
  usage: string;
  main: () => number | void;
}): never {
  try {
    process.exit(opts.main() ?? 0);
  } catch (e) {
    const usageError =
      e instanceof CorpusError ||
      (e instanceof TypeError &&
        String((e as { code?: string }).code).startsWith("ERR_PARSE_ARGS"));
    if (!usageError) throw e;
    process.stderr.write(
      `${opts.tool}: ${(e as Error).message}\nusage: ${opts.usage}\n`,
    );
    process.exit(2);
  }
}
