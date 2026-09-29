// The "typical agent PR" corpus (plan T2.1, design §12.2): 30 changed files
// and ~2k changed lines of TypeScript, Rust, Go and Markdown in a small
// monorepo: 20 modified, 5 added and 3 deleted files, 1 rename with a small
// edit and 1 binary (a PNG), next to 10 untouched files.
//
//   bun benches/corpora/make-typical.ts
//
// Writes <root>/generated/typical (lib.ts has the root) with tags corpus-base
// and corpus-head, and prints its manifest entry as JSON. Deterministic: a
// fixed seed, identity and dates give the same object ids on every run.
import { parseArgs } from "node:util";
import {
  binaryBlob,
  language,
  modifiedPair,
  renamedPair,
  shares,
  sourceLines,
  text,
  weights,
  word,
} from "./content";
import {
  corpusEntry,
  makeRng,
  type ParityEntry,
  printEntry,
  runCli,
  writeCorpus,
} from "./lib";

const seed = 20_260_928;
const changedLines = 2_000;

const areas: Record<string, { dirs: string[]; sep: string }> = {
  ts: {
    dirs: ["web/src/components", "web/src/lib", "web/src/routes"],
    sep: "-",
  },
  rs: {
    dirs: ["crates/engine/src", "crates/engine/src/model", "crates/cli/src"],
    sep: "_",
  },
  go: {
    dirs: ["server/internal/store", "server/internal/api", "server/cmd/app"],
    sep: "_",
  },
  md: { dirs: ["docs", "docs/guides"], sep: "-" },
};

// File extensions per role.
const roles: Record<"same" | "modified" | "added" | "deleted", string[]> = {
  same: ["ts", "ts", "ts", "ts", "rs", "rs", "rs", "go", "go", "md"],
  modified: [
    ...Array<string>(8).fill("ts"),
    ...Array<string>(5).fill("rs"),
    ...Array<string>(4).fill("go"),
    ...Array<string>(3).fill("md"),
  ],
  added: ["ts", "ts", "rs", "go", "md"],
  deleted: ["ts", "rs", "go"],
};

/** Base and head file lists of the typical corpus. */
export function typicalFiles(): { base: ParityEntry[]; head: ParityEntry[] } {
  const rng = makeRng(seed);
  const used = new Set<string>();
  const newPath = (ext: string): string => {
    const area = areas[ext];
    if (!area) throw new Error(`no area for .${ext}`);
    for (;;) {
      const path = `${rng.pick(area.dirs)}/${word(rng)}${area.sep}${word(rng)}.${ext}`;
      if (!used.has(path)) {
        used.add(path);
        return path;
      }
    }
  };
  const base: ParityEntry[] = [];
  const head: ParityEntry[] = [];

  for (const ext of roles.same) {
    const path = newPath(ext);
    const body = text(sourceLines(language(ext), rng, rng.int(40, 220)));
    base.push([path, body]);
    head.push([path, body]);
  }

  // One rename with a small edit: web/src/lib/<x>.ts -> web/src/lib/<dir>/<x>.ts
  const renamed = renamedPair(language("ts"), rng, 120);
  const from = newPath("ts");
  const to = from.replace(/\/([^/]+)$/, `/${word(rng)}/$1`);
  base.push([from, text(renamed.base)]);
  head.push([to, text(renamed.head)]);

  const kinds = [
    ...roles.modified.map((ext) => ({ ext, role: "modified" })),
    ...roles.added.map((ext) => ({ ext, role: "added" })),
    ...roles.deleted.map((ext) => ({ ext, role: "deleted" })),
  ];
  const sizes = shares(
    changedLines - renamed.changed,
    weights(rng, kinds.length),
    12,
  );
  kinds.forEach(({ ext, role }, i) => {
    const lang = language(ext);
    const changed = sizes[i] ?? 12;
    const path = newPath(ext);
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

  // The binary: a logo that grew (12.0 KB -> 14.2 KB).
  base.push(["web/public/logo.png", binaryBlob(rng, 12_288)]);
  head.push(["web/public/logo.png", binaryBlob(rng, 14_540)]);
  return { base, head };
}

if (import.meta.main)
  runCli({
    tool: "make-typical",
    usage: "bun benches/corpora/make-typical.ts",
    main: () => {
      parseArgs({ args: process.argv.slice(2), options: {}, strict: true });
      const entry = corpusEntry("typical");
      writeCorpus({ repo: entry.repo, ...typicalFiles() });
      printEntry(entry);
    },
  });
