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
// the first release) to --commit (default HEAD), which must contain it. Then
// the commits' subjects, newest first, grouped by conventional-commit type:
// Breaking changes (`type!:`), Features (feat), Fixes (fix), Performance
// (perf), Changes (refactor and every other type or subject), Documentation
// (docs) and Maintenance (test, ci, build, chore, style, and any type with an
// internal scope such as ci, release, plan or a milestone M6), folded in a
// <details> block. --sparkle leaves Maintenance unfolded: Sparkle's Markdown
// renderer prints HTML tags as text. An entry is the description with its
// scope in bold, linked to its commit, without a leading plan id (T6.15, S14,
// M5) or a first word that repeats the scope; merge, wip, checkpoint and
// fixup!/squash!/amend! commits are left out, and repeated entries in a
// section share one. A section lists at most 20 entries (10 for a first
// release), then links the rest. The last line links the compare view
// <previous>...v<version>, or for a first release the commit list of
// v<version>. --repo (default dakdevs/polygloss) builds the links.
// Exit codes: 0 written, 1 git failed, 2 usage (and a --previous that
// --commit does not contain: its notes would run backwards).
import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { RELEASE_VERSION } from "./release-version";

const SECTIONS = [
  "Breaking changes",
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
const CONVENTIONAL = /^([A-Za-z]+)(?:\(([^()]*)\))?(!?): +(\S.*)$/;
/** Not a change of its own. */
const SKIPPED = /^(?:(?:wip|checkpoint|merge)\b|(?:fixup|squash|amend)!)/i;
/** Scopes of the repo's own machinery and bookkeeping: Maintenance, whatever the type. */
const INTERNAL =
  /^(?:benches|bun|ci|e2e|parity|perf|plan|release|repo|scripts|workspace|M\d+)$/i;
/** A leading plan id: `T6.15 `, `S14 `, `M5: `. */
const PLAN_ID = /^[TSM]\d+(?:\.\d+)*:? +(?=\S)/;

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
    const [, type = "", scope = "", breaking, description = subject] =
      CONVENTIONAL.exec(subject) ?? [];
    const section = INTERNAL.test(scope)
      ? "Maintenance"
      : breaking
        ? "Breaking changes"
        : (SECTION_OF[type.toLowerCase()] ?? "Changes");
    let text = description.replace(PLAN_ID, "");
    if (scope && text.toLowerCase().startsWith(`${scope.toLowerCase()} `))
      text = text.slice(scope.length).trimStart();
    text = `${scope ? `**${escape(scope)}:** ` : ""}${escape(text)}`;
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
    if (opts.previous) {
      // A newer release's tag (a release that ran first) would make notes
      // from a reversed diff.
      const r = Bun.spawnSync([
        "git",
        "merge-base",
        "--is-ancestor",
        from,
        commit,
      ]);
      if (r.exitCode === 1)
        throw new UsageError(
          `${opts.commit} does not contain ${opts.previous}: no notes since a release that is not its ancestor`,
        );
      if (r.exitCode !== 0)
        throw new Error(`git merge-base failed: ${r.stderr.toString().trim()}`);
    }
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
