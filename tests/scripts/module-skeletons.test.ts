// T1.1 acceptance: every M1 module file exists and opens with a doc line, so
// parallel M1 tasks fill in their own files and never edit shared module lists.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");

const expand = ({ dir, names }: { dir: string; names: string[] }): string[] =>
  names.map((n) => `${dir}/${n}`);

const moduleFiles = [
  ...expand({
    dir: "crates/polygloss-diff/src",
    names: [
      "lib.rs",
      "types.rs",
      "options.rs",
      "lines.rs",
      "hunks.rs",
      "whitespace.rs",
      "word.rs",
      "line_map.rs",
      "rows.rs",
      "unified_text.rs",
    ],
  }),
  ...expand({
    dir: "crates/polygloss-diff/benches",
    names: ["hunks.rs", "word.rs", "line_map.rs"],
  }),
  ...expand({
    dir: "crates/polygloss-core/src",
    names: [
      "lib.rs",
      "ids.rs",
      "paths.rs",
      "testing.rs",
      "objects.rs",
      "urls.rs",
    ],
  }),
  ...expand({
    dir: "crates/polygloss-core/src/git",
    names: [
      "mod.rs",
      "runner.rs",
      "version.rs",
      "repo.rs",
      "resolve.rs",
      "diff_tree.rs",
      "attrs.rs",
      "snapshot.rs",
      "listing.rs",
    ],
  }),
  ...expand({
    dir: "crates/polygloss-core/src/store",
    names: ["mod.rs", "migrations.rs", "events.rs"],
  }),
  ...expand({
    dir: "crates/polygloss-core/src/review",
    names: [
      "mod.rs",
      "models.rs",
      "open.rs",
      "threads.rs",
      "submit.rs",
      "suggestions.rs",
      "viewed.rs",
      "view_state.rs",
      "sessions.rs",
      "summary.rs",
      "carry_forward.rs",
    ],
  }),
  ...expand({
    dir: "crates/polygloss-core/src/ipc",
    names: ["mod.rs", "protocol.rs", "client.rs", "server.rs"],
  }),
];

describe("M1 module skeletons", () => {
  for (const file of moduleFiles) {
    test(`${file} starts with a //! doc line`, () => {
      const first = readFileSync(join(repoRoot, file), "utf8").split("\n")[0];
      expect(first).toMatch(/^\/\/! \S/);
    });
  }

  test("schema-v1.sql exists with a leading comment", () => {
    const sql = readFileSync(
      join(repoRoot, "crates/polygloss-core/src/store/schema-v1.sql"),
      "utf8",
    );
    expect(sql.split("\n")[0]).toMatch(/^-- \S/);
  });

  test("polygloss-diff declares one [[bench]] per bench file", () => {
    const manifest = readFileSync(
      join(repoRoot, "crates/polygloss-diff/Cargo.toml"),
      "utf8",
    );
    for (const name of ["hunks", "word", "line_map"]) {
      expect(manifest).toContain(
        `[[bench]]\nname = "${name}"\nharness = false`,
      );
    }
  });
});
