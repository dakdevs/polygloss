// Deterministic git repos for the hunk parity check (plan T1.16, design §6.3).
//
//   bun scripts/make-parity-repo.ts --seed <n> --files <n> --out <dir>
//
// Builds a repo at <dir> (created; must be missing or empty) with two commits on
// `main`, tagged `parity-base` and `parity-head`, and prints its path. The base
// holds <files> code-like files (Rust, TypeScript, Python, Go, Markdown: nested
// blocks, blank lines, repeated braces); the head applies seeded edits to them
// (insert, delete, move block, re-indent, whitespace-only, CRLF, drop trailing
// newline, token changes, renames) plus a few added and deleted files. The same
// seed and file count always give the same object ids: content comes from a
// seeded PRNG, commits are written by `git fast-import` (no filters, no hooks)
// with a fixed identity and dates, and git runs with no global or system config
// and no inherited GIT_* variables. Nothing here reads this repo's own history.
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** One file of a commit: path, contents and (default `100644`) mode. */
export type ParityEntry = [
  path: string,
  contents: string | Buffer,
  mode?: "100644" | "100755",
];

const epoch = 1_767_225_600; // 2026-01-01T00:00:00Z
const ident = "Polygloss Parity <parity@polygloss.invalid>";

function gitEnv(home: string): Record<string, string> {
  return {
    PATH: process.env.PATH ?? "/usr/bin:/bin",
    HOME: home,
    LC_ALL: "C",
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_TERMINAL_PROMPT: "0",
  };
}

function runGit(opts: {
  cwd: string;
  home: string;
  args: string[];
  stdin?: Buffer;
}): string {
  const r = Bun.spawnSync(["git", ...opts.args], {
    cwd: opts.cwd,
    env: gitEnv(opts.home),
    stdin: opts.stdin ?? "ignore",
  });
  if (r.exitCode !== 0) {
    throw new Error(
      `git ${opts.args.join(" ")} failed (exit ${r.exitCode}) in ${opts.cwd}:\n${r.stderr.toString()}`,
    );
  }
  return r.stdout.toString();
}

/** A `git fast-import` commit that replaces the whole tree with `files`. */
function commitStream(opts: {
  mark: number;
  parent?: number;
  message: string;
  tag: string;
  files: ParityEntry[];
}): Buffer[] {
  const when = `${epoch + opts.mark * 60} +0000`;
  const message = Buffer.from(`${opts.message}\n`);
  const parts: Buffer[] = [
    Buffer.from(
      `commit refs/heads/main\nmark :${opts.mark}\n` +
        `author ${ident} ${when}\ncommitter ${ident} ${when}\n` +
        `data ${message.length}\n`,
    ),
    message,
    Buffer.from(opts.parent ? `from :${opts.parent}\n` : ""),
    Buffer.from("deleteall\n"),
  ];
  const seen = new Set<string>();
  for (const [path, contents, mode = "100644"] of opts.files) {
    if (!path || path.startsWith('"') || /[\n\0]/.test(path) || seen.has(path))
      throw new Error(
        `unsupported or duplicate parity path: ${JSON.stringify(path)}`,
      );
    seen.add(path);
    const data =
      typeof contents === "string" ? Buffer.from(contents) : contents;
    parts.push(Buffer.from(`M ${mode} inline ${path}\ndata ${data.length}\n`));
    parts.push(data, Buffer.from("\n"));
  }
  parts.push(
    Buffer.from(`\nreset refs/tags/${opts.tag}\nfrom :${opts.mark}\n\n`),
  );
  return parts;
}

/**
 * Creates a repo at `out` (missing or empty) whose `parity-base` commit holds
 * exactly `base` and whose `parity-head` commit (its child, checked out on
 * `main`) holds exactly `head`. Bytes are stored as given, CRs included.
 */
export function buildParityRepo(opts: {
  out: string;
  base: ParityEntry[];
  head: ParityEntry[];
}): { path: string } {
  if (existsSync(opts.out) && readdirSync(opts.out).length > 0)
    throw new Error(`--out ${opts.out} is not empty`);
  mkdirSync(opts.out, { recursive: true });
  const path = realpathSync(opts.out);
  const home = mkdtempSync(join(tmpdir(), "polygloss-parity-home-"));
  try {
    const git = (args: string[], stdin?: Buffer) =>
      runGit({ cwd: path, home, args, stdin });
    git(["init", "-q", "--initial-branch=main", "--object-format=sha1", "."]);
    const stream = Buffer.concat([
      ...commitStream({
        mark: 1,
        message: "parity base",
        tag: "parity-base",
        files: opts.base,
      }),
      ...commitStream({
        mark: 2,
        parent: 1,
        message: "parity head",
        tag: "parity-head",
        files: opts.head,
      }),
    ]);
    git(
      ["fast-import", "--quiet", "--done"],
      Buffer.concat([stream, Buffer.from("done\n")]),
    );
    git(["reset", "-q", "--hard", "main"]);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
  return { path };
}

// ---------------------------------------------------------------------------
// Seeded content

/** mulberry32: small, fast, and the same everywhere. */
function prng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

type Rng = {
  next: () => number;
  int: (lo: number, hi: number) => number;
  pick: <T>(items: readonly T[]) => T;
  chance: (p: number) => boolean;
};

function makeRng(seed: number): Rng {
  const next = prng(seed);
  const int = (lo: number, hi: number) =>
    lo + Math.floor(next() * (hi - lo + 1));
  return {
    next,
    int,
    pick: <T>(items: readonly T[]) => items[int(0, items.length - 1)] as T,
    chance: (p: number) => next() < p,
  };
}

const words = [
  "alpha",
  "buffer",
  "cache",
  "delta",
  "entry",
  "field",
  "graph",
  "handle",
  "index",
  "join",
  "key",
  "layout",
  "merge",
  "node",
  "offset",
  "parse",
  "query",
  "range",
  "state",
  "token",
  "update",
  "value",
  "window",
  "count",
  "total",
  "result",
  "items",
  "config",
  "path",
  "line",
  "hunk",
  "block",
] as const;

const languages = ["rs", "ts", "py", "go", "md"] as const;
type Lang = (typeof languages)[number];

const dirs: Record<Lang, readonly string[]> = {
  rs: ["crates/core/src", "crates/core/src/model", "crates/cli/src"],
  ts: ["web/src", "web/src/components", "web/src/lib/util"],
  py: ["tools", "tools/analysis", "tools/analysis/io"],
  go: ["server", "server/internal/store", "server/cmd/app"],
  md: ["docs", "docs/guides", "docs/adr"],
};

function ident2(rng: Rng): string {
  return `${rng.pick(words)}_${rng.pick(words)}`;
}

function camel(rng: Rng): string {
  const w = rng.pick(words);
  const v = rng.pick(words);
  return `${w}${v[0]?.toUpperCase()}${v.slice(1)}`;
}

/** One statement or nested block, `depth` levels deep, for a C-like language. */
function cStatements(
  lang: "rs" | "ts" | "go",
  rng: Rng,
  depth: number,
  indent: string,
): string[] {
  const unit = lang === "go" ? "\t" : "    ";
  const inner = indent + unit;
  const name = lang === "ts" ? camel(rng) : ident2(rng);
  const semi = lang === "go" ? "" : ";";
  const decl = lang === "rs" ? "let" : lang === "ts" ? "const" : "";
  const assign = (v: string, e: string) =>
    lang === "go"
      ? `${indent}${v} := ${e}`
      : `${indent}${decl} ${v} = ${e}${semi}`;
  const kind = depth >= 3 ? 0 : rng.int(0, 5);
  switch (kind) {
    case 1: {
      const cond =
        lang === "rs" || lang === "go"
          ? `${name} > ${rng.int(0, 9)}`
          : `(${name} > ${rng.int(0, 9)})`;
      const lines = [
        `${indent}if ${cond} {`,
        ...cStatements(lang, rng, depth + 1, inner),
      ];
      if (rng.chance(0.5))
        lines.push(
          `${indent}} else {`,
          ...cStatements(lang, rng, depth + 1, inner),
        );
      lines.push(`${indent}}`);
      return lines;
    }
    case 2: {
      const head =
        lang === "rs"
          ? `for item in ${name}.iter() {`
          : lang === "go"
            ? `for _, item := range ${name} {`
            : `for (const item of ${name}) {`;
      return [
        `${indent}${head}`,
        ...cStatements(lang, rng, depth + 1, inner),
        `${inner}${lang === "ts" ? camel(rng) : ident2(rng)} += item.${rng.pick(words)}${semi}`,
        `${indent}}`,
      ];
    }
    case 3:
      return [assign(name, `${rng.pick(words)}(${rng.int(0, 99)})`), ""];
    default:
      return [
        assign(
          name,
          `${rng.pick(words)}.${rng.pick(words)}(${rng.int(0, 99)})`,
        ),
      ];
  }
}

function cItem(lang: "rs" | "ts" | "go", rng: Rng): string[] {
  const name = lang === "ts" ? camel(rng) : ident2(rng);
  const unit = lang === "go" ? "\t" : "    ";
  const body: string[] = [];
  for (let i = rng.int(1, 4); i > 0; i--)
    body.push(...cStatements(lang, rng, 1, unit));
  const ret =
    lang === "rs"
      ? `${unit}${rng.pick(words)}`
      : `${unit}return ${rng.pick(words)}${lang === "ts" ? ";" : ""}`;
  if (rng.chance(0.2)) {
    const fields = Array.from({ length: rng.int(2, 5) }, () =>
      lang === "rs"
        ? `${unit}pub ${ident2(rng)}: u32,`
        : lang === "go"
          ? `${unit}${camel(rng)} int`
          : `${unit}${camel(rng)}: number;`,
    );
    const open =
      lang === "rs"
        ? `pub struct ${camel(rng)} {`
        : lang === "go"
          ? `type ${camel(rng)} struct {`
          : `export type ${camel(rng)} = {`;
    return [open, ...fields, lang === "ts" ? "};" : "}"];
  }
  const sig =
    lang === "rs"
      ? `pub fn ${name}(${rng.pick(words)}: &str) -> u32 {`
      : lang === "go"
        ? `func ${name}(${rng.pick(words)} string) int {`
        : `export function ${name}(${rng.pick(words)}: string): number {`;
  const doc =
    lang === "rs"
      ? `/// Computes the ${rng.pick(words)} ${rng.pick(words)}.`
      : `// ${name} computes the ${rng.pick(words)}.`;
  return [doc, sig, ...body, ret, "}"];
}

function pyItem(rng: Rng): string[] {
  const block = (indent: string, depth: number): string[] => {
    const kind = depth >= 3 ? 0 : rng.int(0, 4);
    const inner = `${indent}    `;
    if (kind === 1)
      return [
        `${indent}if ${ident2(rng)} > ${rng.int(0, 9)}:`,
        ...block(inner, depth + 1),
        ...(rng.chance(0.4)
          ? [`${indent}else:`, ...block(inner, depth + 1)]
          : []),
      ];
    if (kind === 2)
      return [
        `${indent}for item in ${ident2(rng)}:`,
        ...block(inner, depth + 1),
        `${inner}total += item.${rng.pick(words)}`,
      ];
    if (kind === 3)
      return [
        `${indent}${ident2(rng)} = ${rng.pick(words)}(${rng.int(0, 99)})`,
        "",
      ];
    return [
      `${indent}${ident2(rng)} = ${rng.pick(words)}.${rng.pick(words)}(${rng.int(0, 99)})`,
    ];
  };
  if (rng.chance(0.2)) {
    return [
      `class ${camel(rng)}:`,
      `    """The ${rng.pick(words)} model."""`,
      "",
      `    def __init__(self):`,
      ...block("        ", 2),
      "",
      `    def ${ident2(rng)}(self):`,
      ...block("        ", 2),
    ];
  }
  const body: string[] = [];
  for (let i = rng.int(1, 4); i > 0; i--) body.push(...block("    ", 1));
  return [
    `def ${ident2(rng)}(${rng.pick(words)}):`,
    `    """Return the ${rng.pick(words)}."""`,
    ...body,
    `    return ${rng.pick(words)}`,
  ];
}

function mdItem(rng: Rng): string[] {
  const sentence = () =>
    Array.from({ length: rng.int(4, 12) }, () => rng.pick(words)).join(" ") +
    ".";
  switch (rng.int(0, 3)) {
    case 0:
      return [`## ${rng.pick(words)} ${rng.pick(words)}`];
    case 1:
      return Array.from({ length: rng.int(1, 4) }, sentence);
    case 2:
      return Array.from({ length: rng.int(2, 6) }, () => `- ${sentence()}`);
    default:
      return ["```rust", ...cStatements("rs", rng, 1, ""), "```"];
  }
}

function item(lang: Lang, rng: Rng): string[] {
  if (lang === "py") return pyItem(rng);
  if (lang === "md") return mdItem(rng);
  return cItem(lang, rng);
}

type Doc = { lines: string[]; trailingNewline: boolean };

function header(lang: Lang, rng: Rng): string[] {
  switch (lang) {
    case "rs":
      return [
        `//! The ${rng.pick(words)} module.`,
        "",
        `use std::collections::HashMap;`,
      ];
    case "ts":
      return [`import { ${camel(rng)} } from "./${rng.pick(words)}";`];
    case "go":
      return [`package ${rng.pick(words)}`, "", `import "fmt"`];
    case "py":
      return [`"""The ${rng.pick(words)} tools."""`, "", "import os"];
    case "md":
      return [`# ${rng.pick(words)} ${rng.pick(words)}`];
  }
}

function makeDoc(lang: Lang, rng: Rng): Doc {
  const lines = header(lang, rng);
  const target = rng.int(20, 160);
  while (lines.length < target) lines.push("", ...item(lang, rng));
  return { lines, trailingNewline: true };
}

function render(doc: Doc): string {
  const text = doc.lines.join("\n");
  return doc.trailingNewline && doc.lines.length > 0 ? `${text}\n` : text;
}

/** Boundaries between top-level items: indexes of blank lines. */
function blankLines(lines: string[]): number[] {
  const out: number[] = [];
  lines.forEach((l, i) => {
    if (l.trim() === "") out.push(i);
  });
  return out;
}

function randomRange(rng: Rng, len: number, max: number): [number, number] {
  const start = rng.int(0, Math.max(0, len - 1));
  return [start, Math.min(len, start + rng.int(1, max))];
}

const editKinds = [
  "insert",
  "delete",
  "move-block",
  "re-indent",
  "whitespace-only",
  "modify",
  "crlf",
  "drop-trailing-newline",
] as const;

function applyEdit(
  kind: (typeof editKinds)[number],
  lang: Lang,
  doc: Doc,
  rng: Rng,
): void {
  const lines = doc.lines;
  switch (kind) {
    case "insert": {
      const blanks = blankLines(lines);
      const at =
        blanks.length > 0 && rng.chance(0.7)
          ? rng.pick(blanks)
          : rng.int(0, lines.length);
      lines.splice(
        at,
        0,
        ...(rng.chance(0.7)
          ? ["", ...item(lang, rng)]
          : cStatements("rs", rng, 1, "    ")),
      );
      return;
    }
    case "delete": {
      const [a, b] = randomRange(rng, lines.length, 12);
      lines.splice(a, b - a);
      return;
    }
    case "move-block": {
      const [a, b] = randomRange(rng, lines.length, 15);
      const moved = lines.splice(a, b - a);
      lines.splice(rng.int(0, lines.length), 0, ...moved);
      return;
    }
    case "re-indent": {
      const [a, b] = randomRange(rng, lines.length, 10);
      const unit = lang === "go" ? "\t" : lang === "md" ? "  " : "    ";
      const moved = lines.slice(a, b).map((l) => (l === "" ? l : unit + l));
      const wrap: [string[], string[]] =
        lang === "py"
          ? [[`if ${ident2(rng)}:`], []]
          : lang === "md"
            ? [[], []]
            : [
                [
                  lang === "ts"
                    ? `if (${camel(rng)}) {`
                    : `if ${ident2(rng)} {`,
                ],
                ["}"],
              ];
      lines.splice(a, b - a, ...wrap[0], ...moved, ...wrap[1]);
      return;
    }
    case "whitespace-only": {
      for (let n = rng.int(1, 4); n > 0; n--) {
        const i = rng.int(0, Math.max(0, lines.length - 1));
        const l = lines[i];
        if (l === undefined) continue;
        lines[i] = rng.pick([
          `${l}  `,
          l.replace(/^ {4}/, "\t"),
          l.replace(/^\t/, "    "),
          l.replace(/ /, "  "),
          `  ${l}`,
        ]);
      }
      return;
    }
    case "modify": {
      for (let n = rng.int(1, 5); n > 0; n--) {
        const i = rng.int(0, Math.max(0, lines.length - 1));
        const l = lines[i];
        if (l === undefined) continue;
        const from = rng.pick(words);
        lines[i] = l.includes(from)
          ? l.replace(from, rng.pick(words))
          : `${l} ${rng.pick(words)}`;
      }
      return;
    }
    case "crlf": {
      const [a, b] = rng.chance(0.5)
        ? [0, lines.length]
        : randomRange(rng, lines.length, 20);
      for (let i = a; i < b; i++)
        if (!lines[i]?.endsWith("\r")) lines[i] = `${lines[i]}\r`;
      return;
    }
    case "drop-trailing-newline":
      doc.trailingNewline = false;
      return;
  }
}

function renamed(path: string, rng: Rng): string {
  const slash = path.lastIndexOf("/");
  return `${path.slice(0, slash)}/moved-${rng.pick(words)}${path.slice(slash + 1).replace(/^[a-z]+/, "")}`;
}

/**
 * Builds the seeded parity repo at `out`. Returns its path, every path used on
 * either side, and how many times each edit kind (plus `rename`, `add-file` and
 * `delete-file`) was applied.
 */
export function makeParityRepo(opts: {
  seed: number;
  files: number;
  out: string;
}): { path: string; files: string[]; edits: Record<string, number> } {
  const rng = makeRng(opts.seed);
  const edits: Record<string, number> = {};
  const count = (k: string) => {
    edits[k] = (edits[k] ?? 0) + 1;
  };
  const base: ParityEntry[] = [];
  const head: ParityEntry[] = [];
  const newFile = (i: number, prefix: string): [Lang, string] => {
    const lang = languages[i % languages.length] as Lang;
    return [
      lang,
      `${rng.pick(dirs[lang])}/${prefix}${rng.pick(words)}-${i}.${lang}`,
    ];
  };

  for (let i = 0; i < opts.files; i++) {
    const [lang, path] = newFile(i, "");
    const doc = makeDoc(lang, rng);
    if (rng.chance(0.03)) doc.trailingNewline = false;
    base.push([path, render(doc)]);
    const roll = rng.next();
    if (roll < 0.12) {
      head.push([path, render(doc)]);
      continue;
    }
    if (roll < 0.15) {
      count("delete-file");
      continue;
    }
    for (let n = rng.int(1, 3); n > 0; n--) {
      const kind = rng.chance(0.1)
        ? rng.pick(["crlf", "drop-trailing-newline"] as const)
        : rng.pick(editKinds.slice(0, 6));
      applyEdit(kind, lang, doc, rng);
      count(kind);
    }
    let headPath = path;
    if (rng.chance(0.05)) {
      headPath = renamed(path, rng);
      count("rename");
    }
    head.push([headPath, render(doc)]);
  }
  for (let i = 0; i < Math.max(1, Math.round(opts.files * 0.03)); i++) {
    const [lang, path] = newFile(opts.files + i, "added-");
    head.push([path, render(makeDoc(lang, rng))]);
    count("add-file");
  }

  const { path } = buildParityRepo({ out: opts.out, base, head });
  const files = [...new Set([...base, ...head].map(([p]) => p))].sort();
  return { path, files, edits };
}

function usage(message: string): never {
  process.stderr.write(
    `make-parity-repo: ${message}\nusage: bun scripts/make-parity-repo.ts --seed <n> --files <n> --out <dir>\n`,
  );
  process.exit(2);
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const values: Record<string, string> = {};
  for (let i = 0; i < args.length; i += 2) {
    const flag = args[i] ?? "";
    const value = args[i + 1];
    if (!["--seed", "--files", "--out"].includes(flag) || value === undefined)
      usage(`unexpected argument ${JSON.stringify(flag)}`);
    values[flag] = value;
  }
  const seed = Number(values["--seed"]);
  const files = Number(values["--files"]);
  const out = values["--out"];
  if (!Number.isSafeInteger(seed) || seed < 0)
    usage("--seed must be a non-negative integer");
  if (!Number.isSafeInteger(files) || files < 1)
    usage("--files must be a positive integer");
  if (!out) usage("--out is required");
  if (existsSync(out) && readdirSync(out).length > 0)
    usage(`--out ${out} is not empty`);
  process.stdout.write(`${makeParityRepo({ seed, files, out }).path}\n`);
}
