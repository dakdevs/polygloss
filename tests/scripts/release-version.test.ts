// scripts/release-version.ts (ADR-0019): CalVer YYYYMMDD.N in Los Angeles,
// N from the tags on origin (a fake git on PATH here), and an order Sparkle
// agrees with.
import { afterAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { latestTag, nextVersion } from "../../scripts/release-version";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

// Noon on 2026-10-05 in Los Angeles (PDT, UTC-7).
const OCT_5 = new Date("2026-10-05T19:00:00Z");

describe("nextVersion", () => {
  test("the first release of a day is .1", () => {
    expect(nextVersion({ tags: [], now: OCT_5 })).toBe("20261005.1");
  });

  test("a later release of the day is one more than the highest N", () => {
    expect(
      nextVersion({
        tags: ["v20261005.2", "v20261005.1"],
        now: OCT_5,
      }),
    ).toBe("20261005.3");
    expect(
      nextVersion({ tags: ["v20261005.9", "v20261005.10"], now: OCT_5 }),
    ).toBe("20261005.11");
  });

  test("tags of other days and malformed tags do not count", () => {
    expect(
      nextVersion({
        tags: [
          "v20261004.7",
          "v20261006.3",
          "v20261005.01",
          "v20261005.0",
          "v20261005",
          "v20261005.4-rc.1",
          "20261005.5",
          "v1.2.3",
          "v0.1.0",
          "release-20261005.6",
        ],
        now: OCT_5,
      }),
    ).toBe("20261005.1");
  });

  test("the date is Los Angeles', across UTC midnight and both offsets", () => {
    // 23:30 PDT on Oct 5 is already Oct 6 in UTC.
    const lateOct5 = new Date("2026-10-06T06:30:00Z");
    expect(nextVersion({ tags: ["v20261005.1"], now: lateOct5 })).toBe(
      "20261005.2",
    );
    // 00:30 PDT on Oct 6.
    expect(
      nextVersion({
        tags: ["v20261005.1"],
        now: new Date("2026-10-06T07:30:00Z"),
      }),
    ).toBe("20261006.1");
    // PST (UTC-8): 23:30 on Nov 30 is 07:30 UTC on Dec 1.
    expect(
      nextVersion({ tags: [], now: new Date("2026-12-01T07:30:00Z") }),
    ).toBe("20261130.1");
  });
});

describe("latestTag", () => {
  test("the newest release tag by date, then by N as a number", () => {
    expect(
      latestTag(["v20261005.2", "v20261005.10", "v20261004.30", "v20261005.9"]),
    ).toBe("v20261005.10");
    expect(latestTag(["v20261005.10", "v20261006.1"])).toBe("v20261006.1");
  });

  test("malformed tags never count, and no release tag is null", () => {
    expect(latestTag([])).toBeNull();
    expect(
      latestTag([
        "v1.2.3",
        "v20261007.01",
        "20261008.1",
        "v20261009",
        "v20261005.1",
      ]),
    ).toBe("v20261005.1");
    expect(latestTag(["v0.1.0", "v20261005.0"])).toBeNull();
  });
});

/**
 * Sparkle's version order (SUStandardVersionComparator, Sparkle 2): split into
 * runs of digits, of other characters, and single periods; compare part by
 * part, numbers by value, text as strings; a number beats a period or text.
 * When one runs out, a following text part (1.0b1) makes the longer one
 * older, anything else makes it newer.
 */
function sparkleCompare(a: string, b: string): number {
  const kind = (c: string) =>
    c === "." ? "period" : /\d/.test(c) ? "number" : "string";
  const split = (v: string) => {
    const parts: string[] = [];
    for (const c of v) {
      const last = parts.at(-1);
      if (
        last !== undefined &&
        kind(c) !== "period" &&
        kind(last[0]!) === kind(c)
      )
        parts[parts.length - 1] = last + c;
      else parts.push(c);
    }
    return parts;
  };
  const pa = split(a);
  const pb = split(b);
  for (let i = 0; i < Math.min(pa.length, pb.length); i++) {
    const [x, y] = [pa[i]!, pb[i]!];
    const [kx, ky] = [kind(x[0]!), kind(y[0]!)];
    if (kx === ky) {
      if (kx === "number" && Number(x) !== Number(y))
        return Number(x) < Number(y) ? -1 : 1;
      if (kx === "string" && x !== y) return x < y ? -1 : 1;
    } else if (kx === "string" || ky === "string") {
      return kx === "string" ? -1 : 1;
    } else {
      return kx === "number" ? 1 : -1;
    }
  }
  if (pa.length === pb.length) return 0;
  const longer = pa.length > pb.length ? 1 : -1;
  const extra = (pa.length > pb.length ? pa : pb)[
    Math.min(pa.length, pb.length)
  ]!;
  return kind(extra[0]!) === "string" ? -longer : longer;
}

describe("Sparkle's order", () => {
  test("the comparator agrees with orderings Sparkle is known for", () => {
    expect(sparkleCompare("1.0", "1.1")).toBe(-1);
    expect(sparkleCompare("1.0b1", "1.0")).toBe(-1);
    expect(sparkleCompare("1.0", "1.0.1")).toBe(-1);
    expect(sparkleCompare("1.10", "1.9")).toBe(1);
    expect(sparkleCompare("2.0", "2.0")).toBe(0);
  });

  test("successive releases are each newer than the last", () => {
    const versions: string[] = [];
    const tags: string[] = [];
    for (const now of [
      OCT_5,
      ...Array.from({ length: 11 }, () => OCT_5),
      new Date("2026-10-06T19:00:00Z"),
      new Date("2026-10-06T19:00:00Z"),
      new Date("2026-11-01T19:00:00Z"),
      new Date("2027-01-01T19:00:00Z"),
    ]) {
      const v = nextVersion({ tags, now });
      versions.push(v);
      tags.push(`v${v}`);
    }
    expect(versions.slice(0, 3)).toEqual([
      "20261005.1",
      "20261005.2",
      "20261005.3",
    ]);
    expect(versions).toContain("20261005.10");
    expect(versions).toContain("20261006.1");
    for (let i = 1; i < versions.length; i++)
      expect([
        versions[i - 1],
        versions[i],
        sparkleCompare(versions[i - 1]!, versions[i]!),
      ]).toEqual([versions[i - 1], versions[i], -1]);
    // A plain string order would put .10 before .2.
    expect("20261005.10" < "20261005.2").toBe(true);
  });
});

describe("bun scripts/release-version.ts", () => {
  /** Runs the script with a fake `git` that prints `lsRemote` (or fails). */
  function run(
    lsRemote: string | null,
    args: string[] = ["--now", OCT_5.toISOString()],
  ): { code: number; stdout: string; stderr: string; gitArgs: string } {
    const bin = join(
      sandbox.home,
      `bin-${Math.random().toString(36).slice(2)}`,
    );
    mkdirSync(bin);
    const argsLog = join(bin, "git-args");
    writeFileSync(join(bin, "ls-remote.out"), lsRemote ?? "");
    writeFileSync(
      join(bin, "git"),
      `#!/bin/bash\nprintf '%s\\n' "$*" >"${argsLog}"\n${lsRemote === null ? 'echo "fatal: unable to access origin" >&2; exit 128' : `cat "${join(bin, "ls-remote.out")}"`}\n`,
    );
    chmodSync(join(bin, "git"), 0o755);
    const r = Bun.spawnSync(
      ["bun", join(repoRoot, "scripts", "release-version.ts"), ...args],
      {
        cwd: sandbox.home,
        env: { ...sandbox.env, PATH: `${bin}:${process.env.PATH}` },
      },
    );
    return {
      code: r.exitCode ?? -1,
      stdout: r.stdout.toString(),
      stderr: r.stderr.toString(),
      gitArgs: existsSync(argsLog) ? readFileSync(argsLog, "utf8") : "",
    };
  }

  test("reads origin's tags with git ls-remote and prints the next version", () => {
    const r = run(
      [
        "1111111111111111111111111111111111111111\trefs/tags/v20261005.1",
        "2222222222222222222222222222222222222222\trefs/tags/v20261005.2",
        "3333333333333333333333333333333333333333\trefs/tags/v20261004.9",
        "4444444444444444444444444444444444444444\trefs/heads/v20261005.8",
        "",
      ].join("\n"),
    );
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("20261005.3\n");
    expect(r.gitArgs).toBe("ls-remote --tags --refs origin\n");
  });

  test("--previous prints the newest release tag from origin", () => {
    const r = run(
      [
        "1111111111111111111111111111111111111111\trefs/tags/v20261005.9",
        "2222222222222222222222222222222222222222\trefs/tags/v20261005.10",
        "3333333333333333333333333333333333333333\trefs/tags/v1.0.0",
        "",
      ].join("\n"),
      ["--previous"],
    );
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("v20261005.10\n");
    expect(r.gitArgs).toBe("ls-remote --tags --refs origin\n");
    // Before the first release: an empty line.
    expect(run("", ["--previous"]).stdout).toBe("\n");
  });

  test("no tags at all is the day's first release", () => {
    const r = run("");
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("20261005.1\n");
  });

  test("a failing git prints no version and exits 1", () => {
    const r = run(null);
    expect(r.code).toBe(1);
    expect(r.stdout).toBe("");
    expect(r.stderr).toContain("git ls-remote origin failed");
  });

  test("usage errors exit 2", () => {
    expect(run("", ["--now", "yesterday"]).code).toBe(2);
    expect(run("", ["--nope"]).code).toBe(2);
  });
});
