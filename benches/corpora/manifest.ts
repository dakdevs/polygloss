// The perf corpora manifest (plan T2.1): where each of the four design §12.2
// corpora lives and how to open it. run-perf.ts and git-parity.ts use it.
//
//   bun benches/corpora/manifest.ts [--corpus <name>] [--check]
//
// Prints a JSON array of `{ name, repo, base, head, mode }` for typical,
// synthetic, huge-file and linux (or one object with --corpus). Listing reads
// and creates nothing. --check also verifies that each repo exists with both
// revisions; a missing corpus is named on stderr with the command that
// creates it, and the exit code is 1.
import { parseArgs } from "node:util";
import {
  CorpusError,
  type CorpusEntry,
  corpusCommands,
  corpusEntry,
  corpusNames,
  hasCommits,
  isCorpusName,
  runCli,
} from "./lib";

/** All four entries for the given environment. */
export function manifest(
  env: Record<string, string | undefined> = process.env,
): CorpusEntry[] {
  return corpusNames.map((name) => corpusEntry(name, env));
}

if (import.meta.main)
  runCli({
    tool: "manifest",
    usage: "bun benches/corpora/manifest.ts [--corpus <name>] [--check]",
    main: () => {
      const { values } = parseArgs({
        args: process.argv.slice(2),
        options: {
          corpus: { type: "string" },
          check: { type: "boolean" },
        },
        strict: true,
      });
      const only = values.corpus;
      if (only !== undefined && !isCorpusName(only))
        throw new CorpusError(
          `unknown corpus ${JSON.stringify(only)} (expected ${corpusNames.join(", ")})`,
        );
      const entries = only === undefined ? manifest() : [corpusEntry(only)];
      let missing = 0;
      if (values.check)
        for (const e of entries) {
          if (hasCommits(e.repo, [e.base, e.head])) continue;
          missing++;
          process.stderr.write(
            `manifest: the ${e.name} corpus is missing at ${e.repo} (no ${e.base} and ${e.head}); run ${corpusCommands[e.name]}\n`,
          );
        }
      process.stdout.write(
        only === undefined
          ? `${JSON.stringify(entries, null, 2)}\n`
          : `${JSON.stringify(entries[0])}\n`,
      );
      return missing > 0 ? 1 : 0;
    },
  });
