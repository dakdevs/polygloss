// The "synthetic large" corpus (plan T2.1, design §12.2, OQ-P7): 2,000 changed
// files and ~500k diff lines (added plus deleted, as `git diff --numstat`
// counts them) across 30 languages by extension, with 5% renames, ~10% added
// and ~5% deleted files and a few generated lockfiles. File sizes are heavy
// tailed: most files change a few hundred lines, a few several thousand.
//
//   bun benches/corpora/make-synthetic.ts [--scale <s>]
//
// --scale (0.005 to 1, default 1) multiplies the file count and the line
// budget; a scaled corpus goes to <root>/generated/synthetic-scale-<s> so it
// never replaces the full one the manifest points at. Prints the manifest
// entry as JSON. Deterministic for a given scale.
import { parseArgs } from "node:util";
import {
  type Language,
  languages,
  lockfileLines,
  lockfileNames,
  modifiedPair,
  renamedPair,
  shares,
  sourceLines,
  text,
  editLines,
  weights,
  word,
} from "./content";
import {
  CorpusError,
  corpusEntry,
  generatedRepo,
  makeRng,
  type ParityEntry,
  printEntry,
  type Rng,
  runCli,
  writeCorpus,
} from "./lib";

const seed = 500_000;
const fullFiles = 2_000;
const fullLines = 500_000;

/** File and line counts at `scale` (1 = the design §12.2 corpus). */
export function syntheticPlan(scale: number): {
  files: number;
  changedLines: number;
  languages: number;
  renames: number;
  lockfiles: number;
  added: number;
  deleted: number;
  modified: number;
} {
  const files = Math.round(fullFiles * scale);
  const renames = Math.round(files * 0.05);
  const lockfiles = Math.max(1, Math.round(5 * scale));
  const added = Math.round(files * 0.1);
  const deleted = Math.round(files * 0.05);
  return {
    files,
    changedLines: Math.round(fullLines * scale),
    languages: Math.min(languages.length, files - lockfiles),
    renames,
    lockfiles,
    added,
    deleted,
    modified: files - renames - lockfiles - added - deleted,
  };
}

const tops = [
  "packages",
  "services",
  "libs",
  "apps",
  "tools",
  "crates",
  "docs",
  "config",
];

function directories(rng: Rng, n: number): string[] {
  const out = new Set<string>();
  while (out.size < n) {
    const depth = rng.int(1, 3);
    const parts = [rng.pick(tops)];
    for (let i = 0; i < depth; i++) parts.push(word(rng));
    out.add(parts.join("/"));
  }
  return [...out];
}

function fileName(lang: Language, rng: Rng, i: number): string {
  const snake = lang.family.kind === "code" && lang.family.case === "snake";
  const sep = snake ? "_" : "-";
  return `${word(rng)}${sep}${word(rng)}${sep}${i}.${lang.ext}`;
}

function shuffle<T>(rng: Rng, items: T[]): T[] {
  for (let i = items.length - 1; i > 0; i--) {
    const j = rng.int(0, i);
    [items[i], items[j]] = [items[j] as T, items[i] as T];
  }
  return items;
}

/** Base and head file lists of the synthetic corpus at `scale`. */
export function syntheticFiles(scale: number): {
  base: ParityEntry[];
  head: ParityEntry[];
} {
  const plan = syntheticPlan(scale);
  const rng = makeRng(seed);
  const dirs = directories(rng, Math.max(4, Math.round(plan.files / 8)));
  const base: ParityEntry[] = [];
  const head: ParityEntry[] = [];
  let budget = plan.changedLines;

  // Lockfiles: ~0.6% of the budget each, rewritten in 40-line hunks.
  const lockDirs = new Set<string>();
  for (let i = 0; i < plan.lockfiles; i++) {
    const kind = lockfileNames[i % lockfileNames.length] ?? "Cargo.lock";
    let dir = rng.pick(dirs);
    while (lockDirs.has(`${dir}/${kind}`)) dir = rng.pick(dirs);
    lockDirs.add(`${dir}/${kind}`);
    const changed = Math.max(8, Math.round(plan.changedLines * 0.006));
    const before = lockfileLines(kind, rng, changed * 3);
    const after = editLines({
      base: before,
      changed,
      hunks: Math.max(1, Math.round(changed / 40)),
      rng,
      fresh: (n) => lockfileLines(kind, rng, n, false),
    });
    base.push([`${dir}/${kind}`, text(before)]);
    head.push([`${dir}/${kind}`, text(after)]);
    budget -= changed;
  }

  // Everything else, in a shuffled order so languages rotate across roles.
  const roles = shuffle(rng, [
    ...Array<string>(plan.modified).fill("modified"),
    ...Array<string>(plan.added).fill("added"),
    ...Array<string>(plan.deleted).fill("deleted"),
    ...Array<string>(plan.renames).fill("renamed"),
  ]);
  const renameSizes = roles
    .filter((r) => r === "renamed")
    .map(() => rng.int(60, 460));
  // Every other rename is exact (no edit); the rest change ~6% of their lines.
  const renameEdits = renameSizes.reduce(
    (n, lines, i) =>
      n + (i % 2 === 1 ? Math.max(1, Math.round(lines * 0.06)) : 0),
    0,
  );
  const sized = roles.filter((r) => r !== "renamed").length;
  const sizes = shares(budget - renameEdits, weights(rng, sized), 4);

  let sizeIdx = 0;
  let renameIdx = 0;
  roles.forEach((role, i) => {
    const lang = languages[i % languages.length] as Language;
    const path = `${rng.pick(dirs)}/${fileName(lang, rng, i)}`;
    if (role === "renamed") {
      const lines = renameSizes[renameIdx] ?? 100;
      const exact = renameIdx % 2 === 0;
      renameIdx++;
      const to = `${rng.pick(dirs)}/moved-${fileName(lang, rng, i)}`;
      if (exact) {
        const body = text(sourceLines(lang, rng, lines));
        base.push([path, body]);
        head.push([to, body]);
      } else {
        const pair = renamedPair(lang, rng, lines);
        base.push([path, text(pair.base)]);
        head.push([to, text(pair.head)]);
      }
      return;
    }
    const changed = sizes[sizeIdx++] ?? 4;
    if (role === "modified") {
      const pair = modifiedPair(lang, rng, changed);
      base.push([path, text(pair.base)]);
      head.push([path, text(pair.head)]);
    } else if (role === "added") {
      head.push([path, text(sourceLines(lang, rng, changed))]);
    } else {
      base.push([path, text(sourceLines(lang, rng, changed))]);
    }
  });
  return { base, head };
}

function parseScale(raw: string | undefined): number {
  if (raw === undefined) return 1;
  const scale = Number(raw);
  if (!/^\d*\.?\d+$/.test(raw) || !(scale >= 0.005 && scale <= 1))
    throw new CorpusError(
      `--scale must be a number from 0.005 to 1, got ${JSON.stringify(raw)}`,
    );
  return scale;
}

if (import.meta.main)
  runCli({
    tool: "make-synthetic",
    usage: "bun benches/corpora/make-synthetic.ts [--scale <0.005..1>]",
    main: () => {
      const { values } = parseArgs({
        args: process.argv.slice(2),
        options: { scale: { type: "string" } },
        strict: true,
      });
      const scale = parseScale(values.scale);
      const entry = corpusEntry("synthetic");
      if (scale !== 1)
        entry.repo = generatedRepo("synthetic", process.env, `-scale-${scale}`);
      writeCorpus({ repo: entry.repo, ...syntheticFiles(scale) });
      printEntry(entry);
    },
  });
