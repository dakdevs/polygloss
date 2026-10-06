#!/usr/bin/env bun
// The next release version (ADR-0019): CalVer `YYYYMMDD.N`, the date in
// America/Los_Angeles and N one more than the highest N among that date's
// tags `vYYYYMMDD.N` (1 when there is none).
//
//   bun scripts/release-version.ts [--now <ISO 8601 time>] [--previous]
//
// Reads the tags fresh from origin with `git ls-remote`, so
// release.yml, which runs one release at a time, sees the tag the previous
// release pushed. Tags of other dates and malformed ones (`v20261005.01`,
// `v1.2.3`) are ignored. --now replaces the clock. Prints the version without
// the `v`; with --previous, the newest release tag instead (with the `v`;
// an empty line before the first release), which scripts/release-notes.ts
// starts from. Exit codes: 0 printed, 1 git failed, 2 usage.
import { parseArgs } from "node:util";

/** A release version; the tag is `v` + it. */
export const RELEASE_VERSION = /^(\d{8})\.([1-9]\d*)$/;

/** `now`'s date in Los Angeles, `YYYYMMDD`. */
export function releaseDate(now: Date): string {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone: "America/Los_Angeles",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  })
    .format(now)
    .replaceAll("-", "");
}

/** The version a release made at `now` gets, given the existing `tags`. */
export function nextVersion({
  tags,
  now,
}: {
  tags: string[];
  now: Date;
}): string {
  const date = releaseDate(now);
  let last = 0;
  for (const tag of tags) {
    const m = tag.startsWith("v") ? RELEASE_VERSION.exec(tag.slice(1)) : null;
    if (m?.[1] === date) last = Math.max(last, Number(m[2]));
  }
  return `${date}.${last + 1}`;
}

/** The newest release tag among `tags` (by date, then N), or null. */
export function latestTag(tags: string[]): string | null {
  let latest: { tag: string; date: number; n: number } | null = null;
  for (const tag of tags) {
    const m = tag.startsWith("v") ? RELEASE_VERSION.exec(tag.slice(1)) : null;
    if (!m) continue;
    const [date, n] = [Number(m[1]), Number(m[2])];
    if (!latest || date > latest.date || (date === latest.date && n > latest.n))
      latest = { tag, date, n };
  }
  return latest?.tag ?? null;
}

function main(argv: string[]): number {
  let now: Date;
  let previous: boolean;
  try {
    const { values } = parseArgs({
      args: argv,
      options: {
        now: { type: "string" },
        previous: { type: "boolean", default: false },
      },
    });
    now = values.now === undefined ? new Date() : new Date(values.now);
    if (Number.isNaN(now.getTime()))
      throw new Error(`--now wants an ISO 8601 time, not ${values.now}`);
    previous = values.previous;
  } catch (e) {
    console.error(
      `release-version: ${e instanceof Error ? e.message : String(e)}\nusage: bun scripts/release-version.ts [--now <ISO 8601 time>] [--previous]`,
    );
    return 2;
  }
  const r = Bun.spawnSync(["git", "ls-remote", "--tags", "--refs", "origin"], {
    stderr: "inherit",
  });
  if (r.exitCode !== 0) {
    console.error("release-version: git ls-remote origin failed");
    return 1;
  }
  const tags = r.stdout
    .toString()
    .split("\n")
    .flatMap((line) => {
      const ref = line.split("\t")[1] ?? "";
      return ref.startsWith("refs/tags/")
        ? [ref.slice("refs/tags/".length)]
        : [];
    });
  console.log(previous ? (latestTag(tags) ?? "") : nextVersion({ tags, now }));
  return 0;
}

if (import.meta.main) process.exit(main(process.argv.slice(2)));
