// The "single huge file" corpus (plan T2.1, design §12.2): one TypeScript file
// of exactly 200,000 lines at the base, changed in 100 evenly spread hunks
// (about 1,000 changed lines). Over 100k lines per side, so the app shows it
// without syntax until asked (design §12.3).
//
//   bun benches/corpora/make-huge-file.ts
//
// Writes <root>/generated/huge-file (lib.ts has the root) with tags
// corpus-base and corpus-head, and prints its manifest entry as JSON.
// Deterministic: a fixed seed, identity and dates.
import { parseArgs } from "node:util";
import { bodyLines, editLines, language, sourceLines, text } from "./content";
import {
  corpusEntry,
  makeRng,
  type ParityEntry,
  printEntry,
  runCli,
  writeCorpus,
} from "./lib";

const seed = 200_000;
const path = "src/compiler/checker.ts";
export const hugeFileLines = 200_000;
export const hugeFileHunks = 100;

/** Base and head file lists of the huge-file corpus. */
export function hugeFileFiles(): { base: ParityEntry[]; head: ParityEntry[] } {
  const rng = makeRng(seed);
  const ts = language("ts");
  const before = sourceLines(ts, rng, hugeFileLines);
  const after = editLines({
    base: before,
    changed: hugeFileHunks * 10,
    hunks: hugeFileHunks,
    rng,
    fresh: (n) => bodyLines(ts, rng, n),
  });
  return { base: [[path, text(before)]], head: [[path, text(after)]] };
}

if (import.meta.main)
  runCli({
    tool: "make-huge-file",
    usage: "bun benches/corpora/make-huge-file.ts",
    main: () => {
      parseArgs({ args: process.argv.slice(2), options: {}, strict: true });
      const entry = corpusEntry("huge-file");
      writeCorpus({ repo: entry.repo, ...hugeFileFiles() });
      printEntry(entry);
    },
  });
