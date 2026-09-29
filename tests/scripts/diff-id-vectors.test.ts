// Recomputes the golden `diff_id` vectors (fixtures/diff-id-vectors.json) with an
// implementation independent of polygloss-core: design §4.1 / plan OQ-1,
// lowercase_hex(sha256("polygloss/diff/v1\n" + objfmt + "\n" + base_tree + "\n" + head_tree)).
import { afterAll, describe, expect, test } from "bun:test";
import { mkdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const vectorsPath = join(repoRoot, "fixtures/diff-id-vectors.json");

const { vectors } = JSON.parse(readFileSync(vectorsPath, "utf8")) as {
  vectors: {
    object_format: "sha1" | "sha256";
    base_tree: string;
    head_tree: string;
    diff_id: string;
  }[];
};

const emptyTrees = {
  sha1: "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
  sha256: "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321",
} as const;

function diffId({
  objectFormat,
  baseTree,
  headTree,
}: {
  objectFormat: string;
  baseTree: string;
  headTree: string;
}): string {
  const hasher = new Bun.CryptoHasher("sha256");
  hasher.update(`polygloss/diff/v1\n${objectFormat}\n${baseTree}\n${headTree}`);
  return hasher.digest("hex");
}

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

function git({
  cwd,
  args,
  stdin,
}: {
  cwd: string;
  args: string[];
  stdin?: string;
}): string {
  const r = Bun.spawnSync(["git", ...args], {
    cwd,
    env: sandbox.env,
    stdin: stdin === undefined ? "ignore" : Buffer.from(stdin),
  });
  if (r.exitCode !== 0) {
    throw new Error(`git ${args.join(" ")}: ${r.stderr.toString()}`);
  }
  return r.stdout.toString().trim();
}

describe("diff_id golden vectors", () => {
  test("cover both object formats", () => {
    expect(vectors.map((v) => v.object_format).sort()).toEqual([
      "sha1",
      "sha256",
    ]);
  });

  for (const v of vectors) {
    test(`${v.object_format} vector matches an independent sha256`, () => {
      expect(v.diff_id).toMatch(/^[0-9a-f]{64}$/);
      expect(
        diffId({
          objectFormat: v.object_format,
          baseTree: v.base_tree,
          headTree: v.head_tree,
        }),
      ).toBe(v.diff_id);
    });

    test(`${v.object_format} vector trees are the real git trees it documents`, () => {
      const repo = join(sandbox.home, `vectors-${v.object_format}`);
      mkdirSync(repo, { recursive: true });
      git({
        cwd: repo,
        args: ["init", "-q", `--object-format=${v.object_format}`],
      });
      const blob = git({
        cwd: repo,
        args: ["hash-object", "-w", "--stdin"],
        stdin: "hello\n",
      });
      const head = git({
        cwd: repo,
        args: ["mktree", "-z"],
        stdin: `100644 blob ${blob}\tREADME.md\0`,
      });
      const empty = git({ cwd: repo, args: ["mktree"], stdin: "" });
      expect(v.base_tree).toBe(emptyTrees[v.object_format]);
      expect(empty).toBe(v.base_tree);
      expect(head).toBe(v.head_tree);
    });
  }

  test("the Rust golden tests read the same vectors file", () => {
    const rust = readFileSync(
      join(repoRoot, "crates/polygloss-core/tests/ids.rs"),
      "utf8",
    );
    expect(rust).toContain("fixtures/diff-id-vectors.json");
    expect(rust).toContain("fn diff_id_golden_sha1()");
    expect(rust).toContain("fn diff_id_golden_sha256()");
  });
});
