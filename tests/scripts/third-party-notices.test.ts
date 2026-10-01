// Third-party notices (plan T5.7, design §21, G7): scripts/third-party-notices.ts
// renders packaging/third-party-notices.md from `cargo metadata` (license,
// authors and license files of every crate the two shipped binaries link),
// the lumis grammar list, the MPL-2.0 source notice and the bundled assets'
// notices (Lilex OFL, the Pierre theme port's Apache NOTICE). The committed
// file must match the current dependency graph; `cargo tree` (per shipped
// package, its own feature resolution) is the independent oracle.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import {
  buildNotices,
  type Metadata,
  shippedPackages,
} from "../../scripts/third-party-notices";
import { makeSandbox } from "../support/sandbox";

// cargo metadata / cargo tree over the whole workspace.
setDefaultTimeout(120_000);

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts", "third-party-notices.ts");
const noticesPath = join(repoRoot, "packaging", "third-party-notices.md");

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

/** The sandbox plus the real (read-only) toolchain: rustup and the registry. */
const cargoEnv: Record<string, string> = {
  ...sandbox.env,
  CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), ".cargo"),
  RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"),
};

function run(argv: string[]): {
  exitCode: number;
  stdout: string;
  output: string;
} {
  const r = Bun.spawnSync(argv, { env: cargoEnv, cwd: repoRoot });
  const stdout = r.stdout.toString();
  return {
    exitCode: r.exitCode ?? -1,
    stdout,
    output: stdout + r.stderr.toString(),
  };
}

/** `name version → license` of every crate `cargo tree` says `pkg` links. */
function linkedCrates(pkg: string): Map<string, string> {
  const r = run([
    join(repoRoot, "scripts", "cargo.sh"),
    "tree",
    "-p",
    pkg,
    "-e",
    "normal",
    "--target",
    "aarch64-apple-darwin",
    "--prefix",
    "none",
    "--format",
    "{p}\t{l}",
  ]);
  if (r.exitCode !== 0) throw new Error(`cargo tree -p ${pkg}: ${r.output}`);
  const crates = new Map<string, string>();
  for (const line of r.stdout.split("\n")) {
    if (!line.trim()) continue;
    const [p = "", license = ""] = line.split("\t");
    // `name v1.2.3`, `name v1.2.3 (/path)` or `name v1.2.3 (proc-macro)`.
    const [name = "", version = ""] = p.split(" ");
    if (name.startsWith("polygloss-")) continue;
    // A repeated subtree ends in ` (*)`.
    crates.set(
      `${name} ${version.replace(/^v/, "")}`,
      license.replace(/ \(\*\)$/, "").trim(),
    );
  }
  return crates;
}

/** Splits a markdown table row on unescaped `|`. */
function cells(row: string): string[] {
  return row
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split(/(?<!\\)\|/)
    .map((c) => c.trim().replaceAll("\\|", "|"));
}

/** The body of `## <title>` (up to the next `## `). */
function section(md: string, title: string): string {
  const start = md.indexOf(`\n## ${title}\n`);
  if (start < 0) throw new Error(`no "## ${title}" section`);
  const rest = md.slice(start + 1);
  const end = rest.indexOf("\n## ", 1);
  return end < 0 ? rest : rest.slice(0, end);
}

/** Rows of the "Rust crates" table: `name version` → cells. */
function crateRows(md: string): Map<string, string[]> {
  const rows = new Map<string, string[]>();
  for (const line of section(md, "Rust crates").split("\n")) {
    if (!line.startsWith("| ") || line.startsWith("| Crate ")) continue;
    const row = cells(line);
    if (row.every((c) => /^-+$/.test(c))) continue; // header separator
    rows.set(`${row[0]} ${row[1]}`, row);
  }
  return rows;
}

describe("packaging/third-party-notices.md", () => {
  const md = readFileSync(noticesPath, "utf8");

  test("notices cover every shipped crate", () => {
    const rows = crateRows(md);
    const shipped = new Map([
      ...linkedCrates("polygloss-app"),
      ...linkedCrates("polygloss-cli"),
    ]);
    expect(shipped.size).toBeGreaterThan(100);
    const missing = [...shipped.keys()].filter((k) => !rows.has(k));
    expect(missing).toEqual([]);
    // And nothing more: a dev-, build- or perf-only crate is not shipped.
    const extra = [...rows.keys()].filter((k) => !shipped.has(k));
    expect(extra).toEqual([]);
    for (const [crate, license] of shipped) {
      const row = rows.get(crate)!;
      expect({ crate, license: row[2] }).toEqual({ crate, license });
      // Every crate points at the license text(s) that ship for it.
      const refs = [...(row[5] ?? "").matchAll(/\[(L-[0-9a-f]{8})\]/g)].map(
        (m) => m[1],
      );
      expect({ crate, texts: refs.length > 0 }).toEqual({ crate, texts: true });
      for (const ref of refs)
        expect({ crate, ref, found: md.includes(`\n### ${ref}\n`) }).toEqual({
          crate,
          ref,
          found: true,
        });
    }
    // Our own crates are not third-party.
    expect([...rows.keys()].filter((k) => k.startsWith("polygloss"))).toEqual(
      [],
    );
  });

  test("the committed notices are up to date", () => {
    const r = run(["bun", script, "--check"]);
    expect(r.output).toContain("up to date");
    expect(r.exitCode).toBe(0);
  });

  test("carries the bundled assets' notices", () => {
    // Polygloss's own NOTICE (the Pierre theme port's Apache NOTICE, Lilex).
    const notice = readFileSync(join(repoRoot, "NOTICE"), "utf8");
    expect(md).toContain(notice.trim());
    expect(md).toContain("primer/github-vscode-theme");
    // Lilex: the full SIL Open Font License.
    const ofl = readFileSync(
      join(repoRoot, "assets", "fonts", "lilex", "ofl.txt"),
      "utf8",
    );
    expect(section(md, "Lilex font")).toContain(
      ofl.trim().split("\n").slice(-3).join("\n").trim(),
    );
    expect(section(md, "Lilex font")).toContain("SIL OPEN FONT LICENSE");
  });

  test("names where every shipped MPL-2.0 component's source is available", () => {
    const mpl = section(md, "MPL-2.0 components");
    expect(mpl).toContain("nucleo-matcher 0.3.1");
    expect(mpl).toContain("unmodified");
    expect(mpl).toContain("https://crates.io/crates/nucleo-matcher/0.3.1");
    expect(mpl).toContain("https://github.com/helix-editor/nucleo");
    expect(md).toContain("Mozilla Public License Version 2.0");
    // Every MPL-2.0-only crate `cargo tree` says ships has a line.
    const shipped = [
      ...linkedCrates("polygloss-app"),
      ...linkedCrates("polygloss-cli"),
    ];
    const mplOnly = [
      ...new Set(
        shipped.filter(([, l]) => l === "MPL-2.0").map(([crate]) => crate),
      ),
    ];
    expect(mplOnly.length).toBeGreaterThan(0);
    for (const crate of mplOnly)
      expect({ crate, listed: mpl.includes(`- ${crate} (MPL-2.0)`) }).toEqual({
        crate,
        listed: true,
      });
    // The MPL-2.0 files lumis compiles in: helix's wat highlight queries.
    const wat = mpl
      .split("\n")
      .find((l) => l.includes("wat highlight queries"));
    expect(wat).toContain("https://github.com/helix-editor/helix (revision ");
    expect(wat).toContain("unmodified");
  });

  test("lists every lumis grammar with its source and license", () => {
    const grammars = section(md, "Syntax grammars and highlight queries");
    const rows = grammars
      .split("\n")
      .filter((l) => l.startsWith("| ") && !l.startsWith("| Language "))
      .map(cells);
    const byLang = new Map(rows.map((r) => [r[0], r]));
    // One per enabled `lang-*` feature of lumis (Cargo.toml).
    for (const lang of ["rust", "typescript", "yaml", "markdown_inline", "wat"])
      expect({ lang, listed: byLang.has(lang) }).toEqual({
        lang,
        listed: true,
      });
    // Vendored in lumis: upstream repository, revision and license.
    expect(byLang.get("yaml")?.[1]).toContain(
      "https://github.com/ericmj/tree-sitter-yaml",
    );
    expect(byLang.get("yaml")?.[2]).toBe("MIT");
    expect(byLang.get("wat")?.[2]).toBe("Apache-2.0 WITH LLVM-exception");
    // From a crate: named with its version (its license files are under
    // "Rust crates").
    expect(byLang.get("rust")?.[1]).toContain("tree-sitter-rust 0.24.2");
    // Queries: nvim-treesitter unless lumis takes them from elsewhere.
    expect(byLang.get("rust")?.[3]).toContain("nvim-treesitter");
    expect(byLang.get("rust")?.[4]).toBe("Apache-2.0");
    expect(byLang.get("wat")?.[3]).toContain("helix-editor/helix");
    expect(byLang.get("wat")?.[4]).toBe("MPL-2.0");
    for (const row of rows)
      expect({ lang: row[0], cells: row.length }).toEqual({
        lang: row[0],
        cells: 5,
      });
  });
});

// ---- synthetic graphs (no cargo) ------------------------------------------

type Pkg = Metadata["packages"][number];

let fakeCount = 0;
/** A fake crate source dir with `files` (name → contents). */
function fakeCrate(
  name: string,
  opts: {
    version?: string;
    license?: string | null;
    licenseFile?: string;
    files?: Record<string, string>;
    authors?: string[];
  } = {},
): Pkg {
  fakeCount += 1;
  const version = opts.version ?? "1.0.0";
  const dir = join(sandbox.home, `crate-${fakeCount}`, `${name}-${version}`);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "Cargo.toml"), "");
  for (const [file, text] of Object.entries(opts.files ?? {}))
    writeFileSync(join(dir, file), text);
  return {
    id: `registry+https://github.com/rust-lang/crates.io-index#${name}@${version}`,
    name,
    version,
    license: opts.license === undefined ? "MIT" : opts.license,
    license_file: opts.licenseFile ?? null,
    authors: opts.authors ?? [`${name} Author <a@example.invalid>`],
    repository: `https://example.invalid/${name}`,
    homepage: null,
    manifest_path: join(dir, "Cargo.toml"),
    source: "registry+https://github.com/rust-lang/crates.io-index",
  };
}

function member(name: string): Pkg {
  return {
    ...fakeCrate(name, { version: "0.1.0" }),
    id: `path+file:///ws/${name}#0.1.0`,
    source: null,
  };
}

type Edge = [string, string, (string | null)[]];

/** Metadata for `packages` with `edges` (from, to, dep kinds). */
function metadata(packages: Pkg[], edges: Edge[]): Metadata {
  const id = (name: string) => packages.find((p) => p.name === name)!.id;
  return {
    packages,
    workspace_members: packages.filter((p) => !p.source).map((p) => p.id),
    resolve: {
      nodes: packages.map((p) => ({
        id: p.id,
        features: [],
        deps: edges
          .filter(([from]) => from === p.name)
          .map(([, to, kinds]) => ({
            name: to.replaceAll("-", "_"),
            pkg: id(to),
            dep_kinds: kinds.map((kind) => ({ kind, target: null })),
          })),
      })),
    },
  };
}

const MIT_TEXT = "MIT License\n\nCopyright (c) 2024 Someone\n\nPermission…\n";

describe("scripts/third-party-notices.ts", () => {
  test("shipped crates are the normal dependencies of the app and the CLI", () => {
    const meta = metadata(
      [
        member("polygloss-app"),
        member("polygloss-cli"),
        member("polygloss-core"),
        member("polygloss-perf"),
        fakeCrate("linked"),
        fakeCrate("macro"),
        fakeCrate("deep"),
        fakeCrate("build-only"),
        fakeCrate("dev-only"),
        fakeCrate("perf-only"),
        fakeCrate("cli-only"),
      ],
      [
        ["polygloss-app", "polygloss-core", [null]],
        ["polygloss-core", "linked", [null]],
        ["polygloss-app", "macro", [null]],
        ["linked", "deep", [null, "build"]],
        ["polygloss-app", "build-only", ["build"]],
        ["polygloss-app", "dev-only", ["dev"]],
        ["polygloss-perf", "perf-only", [null]],
        ["polygloss-cli", "cli-only", [null]],
      ],
    );
    expect(shippedPackages(meta).map((p) => p.name)).toEqual([
      "cli-only",
      "deep",
      "linked",
      "macro",
    ]);
  });

  test("each crate's own license files ship, identical texts once", () => {
    const meta = metadata(
      [
        member("polygloss-app"),
        member("polygloss-cli"),
        fakeCrate("alpha", {
          license: "MIT OR Apache-2.0",
          files: { "LICENSE-MIT": MIT_TEXT, "LICENSE-APACHE": "Apache text" },
        }),
        fakeCrate("beta", { files: { LICENSE: MIT_TEXT } }),
        fakeCrate("gamma", {
          license: null,
          licenseFile: "COPYING.custom",
          files: { "COPYING.custom": "Custom terms." },
        }),
      ],
      [
        ["polygloss-app", "alpha", [null]],
        ["polygloss-app", "beta", [null]],
        ["polygloss-cli", "gamma", [null]],
      ],
    );
    const md = buildNotices({ metadata: meta, repoRoot });
    const rows = crateRows(md);
    expect(rows.get("alpha 1.0.0")?.[2]).toBe("MIT OR Apache-2.0");
    expect(rows.get("alpha 1.0.0")?.[3]).toBe(
      "alpha Author <a@example.invalid>",
    );
    expect(rows.get("gamma 1.0.0")?.[2]).toBe("see COPYING.custom");
    const refOf = (crate: string) =>
      [...(rows.get(crate)?.[5] ?? "").matchAll(/L-[0-9a-f]{8}/g)].map(
        (m) => m[0],
      );
    // alpha's MIT file and beta's LICENSE are the same text: one section.
    expect(refOf("alpha 1.0.0").length).toBe(2);
    expect(refOf("beta 1.0.0").length).toBe(1);
    expect(refOf("alpha 1.0.0")).toContain(refOf("beta 1.0.0")[0]!);
    expect(md.split("Copyright (c) 2024 Someone").length - 1).toBe(1);
    expect(md).toContain("Custom terms.");
    expect(md).toContain("Apache text");
  });

  test("a crate without a license file gets the standard text of an MIT or Apache-2.0 option", () => {
    const meta = metadata(
      [
        member("polygloss-app"),
        member("polygloss-cli"),
        fakeCrate("bare", { license: "Zlib OR Apache-2.0 OR MIT" }),
        fakeCrate("apache-only", { license: "Apache-2.0" }),
      ],
      [
        ["polygloss-app", "bare", [null]],
        ["polygloss-app", "apache-only", [null]],
      ],
    );
    const md = buildNotices({ metadata: meta, repoRoot });
    const texts = section(md, "License texts");
    expect(texts).toContain("Copyright (c) <year> <copyright holders>");
    expect(texts).toContain("TERMS AND CONDITIONS FOR USE, REPRODUCTION");
    expect(crateRows(md).get("bare 1.0.0")?.[5]).toContain("standard MIT text");
    expect(crateRows(md).get("apache-only 1.0.0")?.[5]).toContain(
      "standard Apache-2.0 text",
    );
  });

  test("a crate with no license file and no MIT or Apache-2.0 option is an error", () => {
    const meta = metadata(
      [
        member("polygloss-app"),
        member("polygloss-cli"),
        fakeCrate("odd", { license: "BSD-3-Clause" }),
      ],
      [["polygloss-app", "odd", [null]]],
    );
    expect(() => buildNotices({ metadata: meta, repoRoot })).toThrow(
      "odd 1.0.0",
    );
  });

  test("MPL-2.0 crates get a source-availability notice", () => {
    const meta = metadata(
      [
        member("polygloss-app"),
        member("polygloss-cli"),
        fakeCrate("weak-copyleft", {
          license: "MPL-2.0",
          files: { LICENSE: "Mozilla Public License Version 2.0\n…" },
        }),
        fakeCrate("either", {
          license: "MIT OR MPL-2.0",
          files: { LICENSE: MIT_TEXT },
        }),
      ],
      [
        ["polygloss-app", "weak-copyleft", [null]],
        ["polygloss-app", "either", [null]],
      ],
    );
    const mpl = section(
      buildNotices({ metadata: meta, repoRoot }),
      "MPL-2.0 components",
    );
    expect(mpl).toContain("weak-copyleft 1.0.0");
    expect(mpl).toContain("https://crates.io/crates/weak-copyleft/1.0.0");
    expect(mpl).toContain("https://example.invalid/weak-copyleft");
    // A permissive alternative is taken instead.
    expect(mpl).not.toContain("either");
  });

  test("an enabled lumis grammar without a known upstream license is an error", () => {
    const lumis = fakeCrate("lumis", {
      version: "0.15.0",
      files: {
        LICENSE: MIT_TEXT,
        "languages.toml": [
          "[queries.default]",
          'git = "https://github.com/nvim-treesitter/nvim-treesitter.git"',
          'rev = "f603a2f4da48728f80257fb5fbb90145fd1dc173"',
          "",
          "[parsers.klingon]",
          'git = "https://example.invalid/tree-sitter-klingon.git"',
          'rev = "0123456789abcdef0123456789abcdef01234567"',
          "",
        ].join("\n"),
      },
    });
    writeFileSync(
      lumis.manifest_path,
      '[package]\nname = "lumis"\n\n[features]\nlang-klingon = []\n',
    );
    const meta = metadata(
      [member("polygloss-app"), member("polygloss-cli"), lumis],
      [["polygloss-app", "lumis", [null]]],
    );
    meta.resolve.nodes.find((n) => n.id === lumis.id)!.features = [
      "lang-klingon",
    ];
    expect(() => buildNotices({ metadata: meta, repoRoot })).toThrow(
      "tree-sitter-klingon",
    );
  });

  test("MPL-2.0 highlight queries lumis compiles in get a source-availability notice", () => {
    const lumis = fakeCrate("lumis", {
      version: "0.15.0",
      files: {
        LICENSE: MIT_TEXT,
        "languages.toml": [
          "[queries.default]",
          'git = "https://github.com/nvim-treesitter/nvim-treesitter.git"',
          'rev = "f603a2f4da48728f80257fb5fbb90145fd1dc173"',
          "",
          "[queries.wat]",
          'git = "https://github.com/helix-editor/helix.git"',
          'rev = "079a789e8cb0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
          "",
          "[parsers.wat]",
          'git = "https://github.com/wasm-lsp/tree-sitter-wasm.git"',
          'rev = "2ca28a9f9d70aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
          "",
          "[parsers.yaml]",
          'git = "https://github.com/ericmj/tree-sitter-yaml.git"',
          'rev = "0123456789abcdef0123456789abcdef01234567"',
          "",
        ].join("\n"),
      },
    });
    writeFileSync(
      lumis.manifest_path,
      '[package]\nname = "lumis"\n\n[features]\nlang-wat = []\nlang-yaml = []\n',
    );
    const meta = metadata(
      [member("polygloss-app"), member("polygloss-cli"), lumis],
      [["polygloss-app", "lumis", [null]]],
    );
    meta.resolve.nodes.find((n) => n.id === lumis.id)!.features = [
      "lang-wat",
      "lang-yaml",
    ];
    const md = buildNotices({ metadata: meta, repoRoot });
    const mpl = section(md, "MPL-2.0 components");
    expect(mpl).not.toContain("None.");
    expect(mpl).toContain(
      "The wat highlight queries (MPL-2.0), compiled into Polygloss by lumis, are used unmodified. The Source Code Form is available at https://github.com/helix-editor/helix (revision 079a789e8cb0).",
    );
    // nvim-treesitter's queries are Apache-2.0: not listed here.
    expect(mpl).not.toContain("yaml");
    expect(mpl).not.toContain("nvim-treesitter");
    // The MPL text ships.
    expect(section(md, "License texts")).toContain(
      "Mozilla Public License Version 2.0",
    );
  });

  test("--check fails on stale notices and rejects unknown arguments", () => {
    const stale = join(sandbox.home, "stale.md");
    writeFileSync(stale, "# Third-party notices\n\nold\n");
    const r = run(["bun", script, "--check", "--out", stale]);
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("out of date");
    expect(r.output).toContain("bun scripts/third-party-notices.ts");
    expect(run(["bun", script, "--bogus"]).exitCode).toBe(2);
  });
});
