// docs/release-readiness.md (plan T5.8) walks the plan's Definition of done:
// every line has a status and evidence, the plan's ticks agree with it, and
// everything left for the user is listed in one place.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");
const read = (path: string) => readFileSync(join(repoRoot, path), "utf8");

const STATUSES = ["done", "manual", "open"] as const;

/** The plan's Definition of done lines: text and whether they are ticked. */
function dodLines(): { text: string; ticked: boolean }[] {
  const plan = read("docs/plan.md");
  const start = plan.indexOf("\n## Definition of done (v1)\n");
  expect(start).toBeGreaterThan(0);
  const end = plan.indexOf("\n---\n", start);
  return plan
    .slice(start, end)
    .split("\n")
    .flatMap((line) => {
      const m = line.match(/^- \[( |x)\] (.+)$/);
      return m ? [{ text: m[2]!, ticked: m[1] === "x" }] : [];
    });
}

/** The readiness doc's DoD entries: `- **[status]** <text> Evidence: …`. */
function readinessEntries(): Map<string, { status: string; evidence: string }> {
  const doc = read("docs/release-readiness.md");
  const entries = new Map<string, { status: string; evidence: string }>();
  for (const block of doc.split("\n- **[").slice(1)) {
    const m = block.match(
      /^(\w+)\]\*\* ([\s\S]+?)\n {2}Evidence: ([\s\S]+?)(\n\n|\n- |$)/,
    );
    if (m) entries.set(m[2]!.trim(), { status: m[1]!, evidence: m[3]!.trim() });
  }
  return entries;
}

/** The section under `## <heading>` up to the next `## `. */
function section(heading: string): string {
  const doc = read("docs/release-readiness.md");
  const start = doc.indexOf(`\n## ${heading}\n`);
  expect(start).toBeGreaterThanOrEqual(0);
  const end = doc.indexOf("\n## ", start + 1);
  return doc.slice(start, end < 0 ? undefined : end);
}

describe("docs/release-readiness.md", () => {
  test("every Definition of done line has a status and evidence", () => {
    const lines = dodLines();
    expect(lines.length).toBeGreaterThanOrEqual(30);
    const entries = readinessEntries();
    for (const { text } of lines) {
      const entry = entries.get(text);
      expect(entry, `no readiness entry for: ${text}`).toBeDefined();
      expect(STATUSES as readonly string[]).toContain(entry!.status);
      expect(entry!.evidence.length).toBeGreaterThan(20);
    }
    // And nothing else: no entry for a line the plan no longer has.
    expect(entries.size).toBe(lines.length);
  });

  test("the plan ticks exactly the lines the readiness doc marks done", () => {
    const entries = readinessEntries();
    for (const { text, ticked } of dodLines())
      expect(
        ticked,
        `plan tick for "${text.slice(0, 60)}…" disagrees with the readiness doc`,
      ).toBe(entries.get(text)?.status === "done");
  });

  test("everything left for the user is listed", () => {
    const manual = section("Left for the user");
    for (let w = 1; w <= 8; w++) expect(manual).toContain(`W${w}`);
    for (const item of [
      "Developer ID Application",
      "5U7E4UQ5M3",
      "GitHub",
      "SPARKLE_PRIVATE_ED_KEY",
      "release.yml",
      "Finder double-click",
      "CGSSessionScreenIsLocked",
      "OQ-P18",
      "screenshot baselines",
    ])
      expect(manual).toContain(item);
    // Every DoD line that is not done says what remains.
    for (const [text, { status, evidence }] of readinessEntries())
      if (status !== "done")
        expect(
          evidence,
          `"${text.slice(0, 40)}…" is ${status} without a "Remaining:" note`,
        ).toContain("Remaining:");
  });

  test("records the cold launch of the release bundle", () => {
    expect(section("Measurements")).toMatch(
      /cold launch[^\n]*median of 5[^\n]*\d+(\.\d+)? ms/i,
    );
  });
});
