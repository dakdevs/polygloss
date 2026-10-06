#!/usr/bin/env bun
// Release notes from the diff (ADR-0019): the Markdown that release.yml
// publishes as the GitHub Release body and embeds in the Sparkle appcast,
// built from git alone (no network, no model), so a release commit always
// gets the same notes.
//
//   bun scripts/release-notes.ts --version <YYYYMMDD.N> [--previous <tag>]
//     [--commit <rev>] [--repo <owner/name>] [--sparkle] [--out <file>]
//
// First a summary line: the commits (merges aside), files changed and lines
// added and removed from --previous (the last release tag; none or empty for
// the first release) to --commit (default HEAD). Then the commits' subjects,
// newest first, grouped by conventional-commit type: Features (feat), Fixes
// (fix), Performance (perf), Changes (refactor and every other type or
// subject), Documentation (docs) and Maintenance (test, ci, build, chore,
// style), folded in a <details> block. --sparkle leaves Maintenance unfolded:
// Sparkle's Markdown renderer prints HTML tags as text. An entry is the
// subject with its scope in bold, linked to its commit; merge, wip,
// checkpoint and fixup!/squash!/amend! commits are left out, and repeated
// subjects in a section share one entry. A section lists at most 20 entries
// (10 for a first release), then links the rest. The last line links the
// compare view <previous>...v<version>, or for a first release the commit
// list of v<version>. --repo (default dakdevs/polygloss) builds the links.
// Exit codes: 0 written, 1 git failed, 2 usage.
import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { RELEASE_VERSION } from "./release-version";

const SECTIONS = [
  "Features",
  "Fixes",
  "Performance",
  "Changes",
  "Documentation",
  "Maintenance",
];
const SECTION_OF: Record<string, string> = {
  feat: "Features",
  fix: "Fixes",
  perf: "Performance",
  docs: "Documentation",
  test: "Maintenance",
  ci: "Maintenance",
  build: "Maintenance",
  chore: "Maintenance",
  style: "Maintenance",
};
/** `type(scope)!: description`; the scope and `!` are optional. */
const CONVENTIONAL = /^([A-Za-z]+)(?:\(([^()]*)\))?!?: +(\S.*)$/;
/** Not a change of its own. */
const SKIPPED = /^(?:(?:wip|checkpoint|merge)\b|(?:fixup|squash|amend)!)/i;

/** `<` outside code spans as `&lt;`, so GitHub shows `<rev>` instead of dropping it. */
function escape(text: string): string {
  return text
    .split("`")
    .map((part, i) => (i % 2 === 1 ? part : part.replaceAll("<", "&lt;")))
    .join("`");
}

function plural(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

/** The notes for `commits` (newest first) and their diff `stat`. */
export function renderNotes({
  version,
  previous,
  repo,
  commits,
  stat,
  sparkle,
}: {
  version: string;
  previous: string | null;
  repo: string;
  commits: { sha: string; subject: string }[];
  stat: { files: number; insertions: number; deletions: number };
  sparkle: boolean;
}): string {
  const base = `https://github.com/${repo}`;
  const tag = `v${version}`;
  const rest = previous
    ? `${base}/compare/${previous}...${tag}`
    : `${base}/commits/${tag}`;
  const limit = previous ? 20 : 10;

  // Section -> entry text -> its commits, in first-seen order.
  const groups = new Map<string, Map<string, string[]>>();
  for (const { sha, subject } of commits) {
    if (SKIPPED.test(subject)) continue;
    const m = CONVENTIONAL.exec(subject);
    const section = (m && SECTION_OF[m[1]!.toLowerCase()]) ?? "Changes";
    const text = m
      ? `${m[2] ? `**${escape(m[2])}:** ` : ""}${escape(m[3]!)}`
      : escape(subject);
    const entries = groups.get(section) ?? new Map<string, string[]>();
    groups.set(section, entries);
    entries.set(text, [...(entries.get(text) ?? []), sha]);
  }

  const counts = `${plural(commits.length, "commit")} · ${plural(stat.files, "file")} changed · +${stat.insertions} −${stat.deletions}`;
  const lines = [
    previous ? `${counts} since ${previous}` : `First release · ${counts}`,
  ];
  for (const section of SECTIONS) {
    const entries = groups.get(section);
    if (!entries) continue;
    const list = [...entries]
      .slice(0, limit)
      .map(
        ([text, shas]) =>
          `- ${text} (${shas.map((s) => `[${s.slice(0, 7)}](${base}/commit/${s})`).join(", ")})`,
      );
    if (entries.size > limit)
      list.push(
        `- and ${entries.size - limit} more ([${previous ? "full diff" : "all commits"}](${rest}))`,
      );
    if (section === "Maintenance" && !sparkle)
      lines.push(
        "",
        "<details>",
        `<summary>Maintenance (${entries.size})</summary>`,
        "",
        ...list,
        "",
        "</details>",
      );
    else lines.push("", `### ${section}`, "", ...list);
  }
  if (groups.size === 0)
    lines.push("", "No changes besides merges and work in progress.");
  lines.push(
    "",
    previous
      ? `**Full diff:** [${previous}...${tag}](${rest})`
      : `**All commits:** [${tag}](${rest})`,
  );
  return `${lines.join("\n")}\n`;
}

class UsageError extends Error {}

/** `git args` in the C locale (shortstat's wording); stdout, or throws. */
function git(args: string[]): string {
  const r = Bun.spawnSync(["git", ...args], {
    env: { ...process.env, LC_ALL: "C" },
  });
  if (r.exitCode !== 0)
    throw new Error(
      `git ${args.join(" ")} failed: ${r.stderr.toString().trim()}`,
    );
  return r.stdout.toString();
}

/** The full commit id `rev` names. */
function commitOf(rev: string): string {
  if (rev.startsWith("-")) throw new UsageError(`not a revision: ${rev}`);
  return git(["rev-parse", "--verify", `${rev}^{commit}`]).trim();
}

function main(argv: string[]): number {
  let opts: {
    version: string;
    previous: string | null;
    commit: string;
    repo: string;
    sparkle: boolean;
    out?: string;
  };
  try {
    const { values } = parseArgs({
      args: argv,
      options: {
        version: { type: "string" },
        previous: { type: "string" },
        commit: { type: "string", default: "HEAD" },
        repo: { type: "string", default: "dakdevs/polygloss" },
        sparkle: { type: "boolean", default: false },
        out: { type: "string" },
      },
    });
    if (!values.version || !RELEASE_VERSION.test(values.version))
      throw new UsageError(
        `--version wants a release version like 20261005.1, not ${JSON.stringify(values.version ?? "")}`,
      );
    if (!/^[\w.-]+\/[\w.-]+$/.test(values.repo))
      throw new UsageError(`--repo wants owner/name, not ${values.repo}`);
    opts = {
      ...values,
      version: values.version,
      previous: values.previous || null,
    };
  } catch (e) {
    console.error(
      `release-notes: ${e instanceof Error ? e.message : String(e)}\nusage: bun scripts/release-notes.ts --version <YYYYMMDD.N> [--previous <tag>] [--commit <rev>] [--repo <owner/name>] [--sparkle] [--out <file>]`,
    );
    return 2;
  }

  let notes: string;
  try {
    const commit = commitOf(opts.commit);
    const from = opts.previous
      ? commitOf(opts.previous)
      : git(["hash-object", "-t", "tree", "/dev/null"]).trim(); // the empty tree
    const log = git([
      "log",
      "--no-merges",
      "--topo-order",
      "--no-show-signature",
      "--format=%H%x00%s",
      opts.previous ? `${from}..${commit}` : commit,
    ]);
    const commits = log
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [sha = "", subject = ""] = line.split("\0");
        return { sha, subject };
      });
    const shortstat = git([
      "diff",
      "--shortstat",
      "--find-renames",
      "--no-color",
      from,
      commit,
    ]);
    const count = (re: RegExp) => Number(shortstat.match(re)?.[1] ?? 0);
    notes = renderNotes({
      ...opts,
      commits,
      stat: {
        files: count(/(\d+) files? changed/),
        insertions: count(/(\d+) insertions?\(\+\)/),
        deletions: count(/(\d+) deletions?\(-\)/),
      },
    });
  } catch (e) {
    console.error(
      `release-notes: ${e instanceof Error ? e.message : String(e)}`,
    );
    return e instanceof UsageError ? 2 : 1;
  }
  if (opts.out) writeFileSync(opts.out, notes);
  else process.stdout.write(notes);
  return 0;
}

if (import.meta.main) process.exit(main(process.argv.slice(2)));
