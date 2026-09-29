// Deterministic git fixture repositories for tests, scripts and manual checks.
//
//   bun scripts/make-fixture-repo.ts <kind> <dir>   # prints the repo path
//
// The repo is created at `<dir>/<kind>` (`dir` is created if missing; the repo
// path must not exist yet). Kind "worktrees" also creates linked worktrees in
// `<dir>/worktrees-linked/`. Git runs with a hermetic environment: no global or
// system config, fixed identity, fixed per-commit dates, and no inherited
// GIT_DIR-style overrides, so the same kind always yields the same object ids.
import { existsSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const kinds = [
  "basic",
  "renames",
  "hostile-paths",
  "worktrees",
  "sha256",
  "unborn",
] as const;

type FixtureKind = (typeof kinds)[number];

/** A file tree: path (string, or raw bytes for non-UTF-8 names) → contents. */
type Files = Array<[path: string | Buffer, contents: string]>;

const epoch = 1_767_225_600; // 2026-01-01T00:00:00Z

function hermeticEnv(opts: {
  home: string;
  commit: number;
}): Record<string, string> {
  const date = `@${epoch + opts.commit * 60} +0000`;
  return {
    PATH: process.env.PATH ?? "/usr/bin:/bin",
    HOME: opts.home,
    LC_ALL: "C",
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_TERMINAL_PROMPT: "0",
    GIT_AUTHOR_NAME: "Polygloss Fixture",
    GIT_AUTHOR_EMAIL: "fixture@polygloss.invalid",
    GIT_AUTHOR_DATE: date,
    GIT_COMMITTER_NAME: "Polygloss Fixture",
    GIT_COMMITTER_EMAIL: "fixture@polygloss.invalid",
    GIT_COMMITTER_DATE: date,
  };
}

/** Runs git in `cwd`; returns raw stdout, throws with stderr on failure. */
function runGit(opts: {
  cwd: string;
  home: string;
  args: string[];
  stdin?: Buffer;
  commit?: number;
}): Buffer {
  const r = Bun.spawnSync(["git", ...opts.args], {
    cwd: opts.cwd,
    env: hermeticEnv({ home: opts.home, commit: opts.commit ?? 0 }),
    stdin: opts.stdin ?? "ignore",
  });
  if (r.exitCode !== 0) {
    throw new Error(
      `git ${opts.args.join(" ")} failed (exit ${r.exitCode}) in ${opts.cwd}:\n${r.stderr.toString()}`,
    );
  }
  return r.stdout;
}

function lines(prefix: string, count: number): string {
  let out = "";
  for (let i = 1; i <= count; i++) out += `${prefix} line ${i}\n`;
  return out;
}

function writeFiles(root: string, files: Files): void {
  for (const [path, contents] of files) {
    const full = join(root, path.toString());
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, contents);
  }
}

const basicV1: Files = [
  ["README.md", "# Fixture\n\nA small repository for tests.\n"],
  ["src/main.rs", 'fn main() {\n    println!("hello");\n}\n'],
  ["src/lib.rs", lines("lib", 12)],
  ["docs/old.md", lines("old", 5)],
];

const basicV2: Files = [
  ["README.md", "# Fixture\n\nA small repository for tests.\n"],
  [
    "src/main.rs",
    'fn main() {\n    println!("hello, world");\n    run();\n}\n',
  ],
  ["src/lib.rs", lines("lib", 12)],
  ["src/util.rs", "pub fn run() {}\n"],
];

const renamesV1: Files = [
  ["README.md", "# Renames\n"],
  ["src/alpha.rs", lines("alpha", 20)],
  ["src/beta.rs", lines("beta", 20)],
  ["src/gamma.rs", lines("gamma", 20)],
];

const renamesV2: Files = [
  ["README.md", "# Renames\n\nFiles moved around.\n"],
  // Pure rename (R100), rename with edits (similarity < 100), unchanged file.
  ["lib/alpha.rs", lines("alpha", 20)],
  [
    "src/beta-renamed.rs",
    lines("beta", 20).replace("beta line 3\n", "beta line three\n"),
  ],
  ["src/gamma.rs", lines("gamma", 20)],
];

// Hostile names: spaces, quotes, tabs, newlines, backslashes, a leading dash,
// non-ASCII UTF-8 and a name that is not UTF-8 at all (APFS refuses to create
// it, so that one lives in the tree and index only, marked skip-worktree).
const notUtf8 = Buffer.concat([
  Buffer.from("latin1-"),
  Buffer.from([0xe9, 0xff]),
  Buffer.from(".txt"),
]);

const hostileV1: Files = [
  ["plain.txt", lines("plain", 10)],
  ["with space.txt", lines("space", 10)],
  ['quote".txt', lines("quote", 10)],
  ["tab\there.txt", lines("tab", 10)],
  ["tab\tmove-out.txt", lines("tab-out", 10)],
  ["new\nline.txt", lines("newline", 10)],
  ["back\\slash.txt", lines("backslash", 10)],
  ["-leading-dash.txt", lines("dash", 10)],
  ["café/naïve.md", lines("naive", 10)],
  ["日本語.txt", lines("nihongo", 10)],
  [notUtf8, lines("latin1", 10)],
];

const hostileV2: Files = [
  // plain.txt renamed into a hostile path; a tab path renamed out of one.
  ["moved\ninto newline.txt", lines("plain", 10)],
  ["out-of-tab.txt", lines("tab-out", 10)],
  ["tab\there.txt", lines("tab", 10)],
  ["with space.txt", lines("space", 10) + "space line 11\n"],
  ['quote".txt', lines("quote", 10)],
  ["new\nline.txt", lines("newline", 10)],
  ["back\\slash.txt", lines("backslash", 10)],
  ["-leading-dash.txt", lines("dash", 10)],
  ["café/naïve.md", lines("naive", 10).replace("line 5", "line five")],
  ["日本語.txt", lines("nihongo", 10)],
  [notUtf8, lines("latin1", 10) + "latin1 line 11\n"],
];

function isUtf8(path: string | Buffer): boolean {
  if (typeof path === "string") return true;
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(path);
    return true;
  } catch {
    return false;
  }
}

export function makeFixtureRepo(opts: {
  dir: string;
  kind:
    "basic" | "renames" | "hostile-paths" | "worktrees" | "sha256" | "unborn";
}): { path: string; git: (args: string[]) => string } {
  if (!kinds.includes(opts.kind))
    throw new Error(`unknown fixture kind: ${String(opts.kind)}`);
  mkdirSync(opts.dir, { recursive: true });
  const dir = realpathSync(opts.dir);
  const path = join(dir, opts.kind);
  if (existsSync(path)) throw new Error(`fixture path already exists: ${path}`);
  let commits = 0;

  const raw = (args: string[], extra?: { cwd?: string; stdin?: Buffer }) =>
    runGit({
      cwd: extra?.cwd ?? path,
      home: dir,
      args,
      stdin: extra?.stdin,
      commit: commits,
    });
  const git = (args: string[]) => raw(args).toString();

  const init = (objectFormat: "sha1" | "sha256") =>
    runGit({
      cwd: dir,
      home: dir,
      args: [
        "init",
        "-q",
        "--initial-branch=main",
        `--object-format=${objectFormat}`,
        path,
      ],
    });

  /** Porcelain commit of exactly `files` (the worktree is replaced). */
  const commitFiles = (files: Files, message: string, cwd = path) => {
    for (const [file] of files) {
      if (!isUtf8(file)) throw new Error("porcelain fixtures need UTF-8 paths");
    }
    raw(["rm", "-rq", "--cached", "--ignore-unmatch", "."], { cwd });
    raw(["clean", "-fdq"], { cwd });
    writeFiles(cwd, files);
    raw(["add", "-A"], { cwd });
    raw(["commit", "-q", "-m", message], { cwd });
    commits++;
  };

  /** Plumbing commit of exactly `files`; works for any byte path. */
  const commitTree = (files: Files, message: string) => {
    let info = Buffer.alloc(0);
    for (const [file, contents] of files) {
      const oid = raw(["hash-object", "-w", "--stdin"], {
        stdin: Buffer.from(contents),
      })
        .toString()
        .trim();
      const name = typeof file === "string" ? Buffer.from(file) : file;
      info = Buffer.concat([
        info,
        Buffer.from(`100644 blob ${oid}\t`),
        name,
        Buffer.from([0]),
      ]);
    }
    raw(["read-tree", "--empty"]);
    raw(["update-index", "--add", "-z", "--index-info"], { stdin: info });
    const tree = git(["write-tree"]).trim();
    const parentArgs =
      commits > 0 ? ["-p", git(["rev-parse", "HEAD^{commit}"]).trim()] : [];
    const commit = git([
      "commit-tree",
      tree,
      ...parentArgs,
      "-m",
      message,
    ]).trim();
    raw(["update-ref", "HEAD", commit]);
    commits++;
  };

  switch (opts.kind) {
    case "basic":
    case "sha256":
      init(opts.kind === "sha256" ? "sha256" : "sha1");
      commitFiles(basicV1, "Initial commit");
      commitFiles(basicV2, "Edit main, add util, drop old docs");
      break;
    case "renames":
      init("sha1");
      commitFiles(renamesV1, "Initial commit");
      commitFiles(renamesV2, "Move files around");
      break;
    case "hostile-paths": {
      init("sha1");
      commitTree(hostileV1, "Initial commit with hostile paths");
      commitTree(hostileV2, "Rename into and out of hostile paths");
      // Populate the checkout with every path the filesystem accepts.
      let skip = Buffer.alloc(0);
      for (const [file] of hostileV2) {
        if (!isUtf8(file))
          skip = Buffer.concat([skip, file as Buffer, Buffer.from([0])]);
      }
      raw(["update-index", "--skip-worktree", "-z", "--stdin"], {
        stdin: skip,
      });
      raw(["checkout-index", "-a", "-f"]);
      break;
    }
    case "worktrees": {
      init("sha1");
      commitFiles(basicV1, "Initial commit");
      commitFiles(basicV2, "Edit main, add util, drop old docs");
      const linked = join(dir, "worktrees-linked");
      const feature = join(linked, "feature");
      raw(["worktree", "add", "-q", "-b", "feature", feature, "main"]);
      commitFiles(
        [...basicV2, ["src/feature.rs", "pub fn feature() {}\n"]],
        "Add feature",
        feature,
      );
      raw([
        "worktree",
        "add",
        "-q",
        "--detach",
        join(linked, "detached"),
        "main~1",
      ]);
      break;
    }
    case "unborn":
      init("sha1");
      writeFiles(path, [
        ["README.md", "# Unborn\n"],
        ["notes.txt", "untracked\n"],
      ]);
      raw(["add", "README.md"]);
      break;
  }

  return { path, git };
}

if (import.meta.main) {
  const [kind, dir] = process.argv.slice(2);
  if (!kind || !dir || !(kinds as readonly string[]).includes(kind)) {
    process.stderr.write(
      `usage: bun scripts/make-fixture-repo.ts <${kinds.join("|")}> <dir>\n`,
    );
    process.exit(2);
  }
  const repo = makeFixtureRepo({ dir, kind: kind as FixtureKind });
  process.stdout.write(`${repo.path}\n`);
}
