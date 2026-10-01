#!/usr/bin/env bun
// Third-party notices for the bundle (plan T5.7, design §21, G7):
//
//   bun scripts/third-party-notices.ts [--check] [--out <file>]
//
// Renders packaging/third-party-notices.md (or --out), which
// scripts/package-release.sh copies into Polygloss.app/Contents/Resources:
//
//   - Polygloss's own NOTICE (the Pierre theme port's Apache-2.0 NOTICE,
//     Lilex) and Lilex's SIL Open Font License;
//   - the MPL-2.0 crates (nucleo-matcher) with where their source is;
//   - the tree-sitter grammars and highlight queries lumis compiles in, with
//     their upstream licenses;
//   - every crate the shipped binaries link (`cargo metadata`: normal
//     dependencies of polygloss-app and polygloss-cli on aarch64-apple-darwin,
//     our own crates excluded) with its license and authors, and the license
//     files of its published package, each distinct text printed once. A
//     crate that publishes none gets the standard text of its MIT or
//     Apache-2.0 option (packaging/license-texts/, LICENSE-APACHE).
//
// --check exits 1 when the file differs from what the current graph renders
// (CI's audit job; tests/scripts/third-party-notices.test.ts). Usage errors
// exit 2.
import { createHash } from "node:crypto";
import {
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";

/** The notices file, relative to the repo root. */
export const NOTICES_PATH = "packaging/third-party-notices.md";
/** The packages whose binaries ship in Polygloss.app. */
export const SHIPPED_ROOTS = ["polygloss-app", "polygloss-cli"];
/** The only platform v1 ships on (ADR-0016). */
export const TARGET = "aarch64-apple-darwin";

/**
 * Upstream licenses of the grammar and query sources lumis vendors (they
 * ship no license files of their own). Checked against GitHub's license API
 * on 2026-09-30; an enabled grammar or query source missing here fails the
 * run, so a lumis bump or a new `lang-*` feature forces a fresh check.
 */
const UPSTREAM_LICENSES: Record<string, string> = {
  "https://github.com/ericmj/tree-sitter-bash": "MIT",
  "https://github.com/tree-sitter-grammars/tree-sitter-diff": "MIT",
  "https://github.com/camdencheek/tree-sitter-dockerfile": "MIT",
  "https://github.com/leandrocp/tree-sitter-html": "MIT",
  "https://github.com/fwcd/tree-sitter-kotlin": "MIT",
  "https://github.com/coder3101/tree-sitter-proto": "MIT",
  "https://github.com/ericmj/tree-sitter-python": "MIT",
  "https://github.com/ericmj/tree-sitter-ruby": "MIT",
  "https://github.com/serenadeai/tree-sitter-scss": "MIT",
  "https://github.com/leandrocp/tree-sitter-svelte": "MIT",
  "https://github.com/tree-sitter-grammars/tree-sitter-vue": "MIT",
  "https://github.com/wasm-lsp/tree-sitter-wasm":
    "Apache-2.0 WITH LLVM-exception",
  "https://github.com/ericmj/tree-sitter-xml": "MIT",
  "https://github.com/ericmj/tree-sitter-yaml": "MIT",
  // Highlight queries.
  "https://github.com/nvim-treesitter/nvim-treesitter": "Apache-2.0",
  "https://github.com/tree-sitter/tree-sitter-python": "MIT",
  "https://github.com/tree-sitter-grammars/tree-sitter-make": "MIT",
  "https://github.com/nix-community/tree-sitter-nix": "MIT",
  "https://github.com/derekstride/tree-sitter-sql": "MIT",
  "https://github.com/helix-editor/helix": "MPL-2.0",
};

type DepKind = { kind: string | null; target: string | null };

/** The parts of `cargo metadata --format-version 1` this script reads. */
export type Metadata = {
  packages: {
    id: string;
    name: string;
    version: string;
    license: string | null;
    license_file: string | null;
    authors: string[];
    repository: string | null;
    homepage: string | null;
    manifest_path: string;
    source: string | null;
  }[];
  workspace_members: string[];
  resolve: {
    nodes: {
      id: string;
      features: string[];
      deps: { name: string; pkg: string; dep_kinds: DepKind[] }[];
    }[];
  };
};

type Package = Metadata["packages"][number];

/** One license text: its id (`L-` + 8 hex of its SHA-256), body and users. */
type Text = { id: string; body: string; users: string[] };

/**
 * The third-party packages reachable from {@link SHIPPED_ROOTS} through
 * normal dependencies (proc macros included; build scripts and dev
 * dependencies never ship), sorted by name and version.
 */
export function shippedPackages(
  metadata: Metadata,
  roots: string[] = SHIPPED_ROOTS,
): Package[] {
  const byId = new Map(metadata.packages.map((p) => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const members = new Set(metadata.workspace_members);
  const queue = metadata.packages
    .filter((p) => roots.includes(p.name) && members.has(p.id))
    .map((p) => p.id);
  if (queue.length !== roots.length)
    throw new Error(
      `cargo metadata lacks a shipped package (${roots.join(", ")})`,
    );
  const seen = new Set(queue);
  while (queue.length > 0) {
    const node = nodes.get(queue.pop()!);
    for (const dep of node?.deps ?? []) {
      if (!dep.dep_kinds.some((k) => k.kind === null)) continue;
      if (seen.has(dep.pkg)) continue;
      seen.add(dep.pkg);
      queue.push(dep.pkg);
    }
  }
  return [...seen]
    .filter((id) => !members.has(id))
    .map((id) => byId.get(id)!)
    .sort(
      (a, b) =>
        a.name.localeCompare(b.name) ||
        a.version.localeCompare(b.version, undefined, { numeric: true }),
    );
}

/** Line endings, trailing spaces and surrounding blank lines normalized. */
function normalize(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((l) => l.trimEnd())
    .join("\n")
    .replace(/^\n+/, "")
    .replace(/\n+$/, "");
}

/**
 * A text's id: `L-` + 8 hex of the SHA-256 of its words (whitespace runs
 * collapsed), so copies that differ only in wrapping or indentation (common
 * among Apache-2.0 files) print once.
 */
function textId(body: string): string {
  const words = body.replace(/\s+/g, " ").trim();
  return `L-${createHash("sha256").update(words).digest("hex").slice(0, 8)}`;
}

const LICENSE_FILE =
  /^(licen[cs]e|copying|copyright|notice|unlicense)([-._].*)?$/i;

/** The license files a crate publishes: `LICENSE*`, `COPYING*`, `NOTICE*`, … */
function licenseFiles(pkg: Package): { name: string; text: string }[] {
  const dir = dirname(pkg.manifest_path);
  const files: { name: string; text: string }[] = [];
  const add = (path: string) => {
    const name = relative(dir, path);
    if (files.some((f) => f.name === name)) return;
    files.push({ name, text: readFileSync(path, "utf8") });
  };
  for (const entry of readdirSync(dir).sort()) {
    if (!LICENSE_FILE.test(entry)) continue;
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      // REUSE-style `LICENSES/<id>.txt`.
      for (const inner of readdirSync(path).sort())
        if (statSync(join(path, inner)).isFile()) add(join(path, inner));
    } else {
      add(path);
    }
  }
  if (pkg.license_file) {
    const path = resolve(dir, pkg.license_file);
    if (!existsSync(path))
      throw new Error(
        `${pkg.name} ${pkg.version}: license-file ${pkg.license_file} is missing`,
      );
    add(path);
  }
  return files;
}

/** The SPDX ids an expression offers (`MIT OR Apache-2.0`, `MIT/Apache-2.0`). */
function licenseOptions(expr: string): string[] {
  return expr
    .replace(/[()]/g, " ")
    .split(/\s+OR\s+|\//)
    .map((s) => s.trim())
    .filter(Boolean);
}

/** Standard texts for licenses whose holders ship no file of their own. */
function standardTexts(repoRoot: string): Record<string, string> {
  const read = (...p: string[]) => readFileSync(join(repoRoot, ...p), "utf8");
  const apache = read("LICENSE-APACHE");
  return {
    MIT: read("packaging", "license-texts", "mit.txt"),
    "Apache-2.0": apache,
    "Apache-2.0 WITH LLVM-exception": `${normalize(apache)}\n\n${read("packaging", "license-texts", "llvm-exception.txt")}`,
    "MPL-2.0": read("packaging", "license-texts", "mpl-2.0.txt"),
  };
}

/** Collects license texts, each distinct body once. */
class Texts {
  readonly byId = new Map<string, Text>();

  /** Adds `raw` for `user`; returns a markdown link to its section. */
  link(raw: string, user: string): string {
    const body = normalize(raw);
    const id = textId(body);
    const text = this.byId.get(id) ?? { id, body, users: [] };
    if (!text.users.includes(user)) text.users.push(user);
    this.byId.set(id, text);
    return `[${id}](#${id.toLowerCase()})`;
  }
}

type Grammar = {
  language: string;
  /** Vendored in lumis (no license file ships with it). */
  vendored: boolean;
  grammar: string;
  license: string;
  queries: string;
  queriesLicense: string;
};

function gitUrl(git: string): string {
  return git.replace(/\.git$/, "");
}

function upstreamLicense(url: string, what: string): string {
  const license = UPSTREAM_LICENSES[url];
  if (!license)
    throw new Error(
      `no known license for ${what} (${url}): look it up and add it to UPSTREAM_LICENSES in scripts/third-party-notices.ts`,
    );
  return license;
}

/**
 * The grammars lumis compiles in: one per enabled `lang-*` feature (bundles
 * expanded), from a crate or vendored in lumis (languages.toml names the
 * upstream repository and revision), with the source of its highlight
 * queries.
 */
function lumisGrammars(metadata: Metadata, shipped: Package[]): Grammar[] {
  const lumis = shipped.find((p) => p.name === "lumis");
  if (!lumis) return [];
  const dir = dirname(lumis.manifest_path);
  const manifest = Bun.TOML.parse(
    readFileSync(lumis.manifest_path, "utf8"),
  ) as {
    features?: Record<string, string[]>;
  };
  const toml = Bun.TOML.parse(
    readFileSync(join(dir, "languages.toml"), "utf8"),
  ) as {
    parsers?: Record<string, { git?: string; rev?: string }>;
    queries?: Record<string, { git?: string; rev?: string }>;
  };
  const node = metadata.resolve.nodes.find((n) => n.id === lumis.id);
  const features = manifest.features ?? {};
  const byId = new Map(metadata.packages.map((p) => [p.id, p]));
  const depPackage = (crate: string) =>
    node?.deps
      .map((d) => byId.get(d.pkg))
      .find((p) => p?.name === crate && shipped.includes(p));

  const langs = (node?.features ?? []).filter(
    (f) => /^lang-/.test(f) && !/^lang-bundle-/.test(f),
  );
  const defaultQueries = toml.queries?.default;
  return langs.sort().map((feature) => {
    const language = feature.replace(/^lang-/, "").replaceAll("-", "_");
    const crate = (features[feature] ?? [])
      .find((v) => v.startsWith("dep:"))
      ?.slice(4);
    let grammar: string;
    let license: string;
    if (crate) {
      const pkg = depPackage(crate);
      if (!pkg)
        throw new Error(`lumis ${feature} needs ${crate}, which is not linked`);
      grammar = `crate ${pkg.name} ${pkg.version}`;
      license = pkg.license ?? `see ${pkg.name}'s license file`;
    } else {
      const parser = toml.parsers?.[language];
      if (!parser?.git || !parser.rev)
        throw new Error(`lumis languages.toml has no parser for ${feature}`);
      const url = gitUrl(parser.git);
      grammar = `vendored in lumis ${lumis.version} from ${url} @ ${parser.rev.slice(0, 12)}`;
      license = upstreamLicense(url, `the ${language} grammar`);
    }
    const q = toml.queries?.[language] ?? defaultQueries;
    if (!q?.git) throw new Error(`lumis languages.toml has no default queries`);
    const qUrl = gitUrl(q.git);
    return {
      language,
      grammar,
      license,
      vendored: !crate,
      queries: `${qUrl} @ ${(q.rev ?? "").slice(0, 12)}`,
      queriesLicense: upstreamLicense(qUrl, `the ${language} queries`),
    };
  });
}

function cell(value: string): string {
  return value.replace(/\s+/g, " ").replaceAll("|", "\\|").trim() || "—";
}

function fenced(body: string): string {
  const longest = Math.max(
    0,
    ...[...body.matchAll(/~+/g)].map((m) => m[0].length),
  );
  const fence = "~".repeat(Math.max(4, longest + 1));
  return `${fence}text\n${body}\n${fence}`;
}

function crateUrl(pkg: Package): string {
  return `https://crates.io/crates/${pkg.name}/${pkg.version}`;
}

/**
 * The notices markdown for `metadata` (file contents under `repoRoot`).
 * `linked` (`name version` keys, from {@link linkedCrates}) narrows the
 * crates to those the shipped binaries really link: `cargo metadata`
 * resolves features for the whole workspace at once, so its graph also has
 * optional dependencies only dev and perf builds turn on.
 */
export function buildNotices(opts: {
  metadata: Metadata;
  repoRoot: string;
  linked?: Set<string>;
}): string {
  const { metadata, repoRoot, linked } = opts;
  const shipped = shippedPackages(metadata).filter(
    (p) => !linked || linked.has(`${p.name} ${p.version}`),
  );
  const standard = standardTexts(repoRoot);
  const texts = new Texts();

  const rows = shipped.map((pkg) => {
    const who = `${pkg.name} ${pkg.version}`;
    if (!pkg.license && !pkg.license_file)
      throw new Error(`${who} declares no license`);
    const refs = licenseFiles(pkg).map((f) =>
      texts.link(f.text, `${who} (${f.name})`),
    );
    if (refs.length === 0) {
      const options = licenseOptions(pkg.license ?? "");
      const pick = ["MIT", "Apache-2.0"].find((l) => options.includes(l));
      if (!pick)
        throw new Error(
          `${who} publishes no license file and offers neither MIT nor Apache-2.0 (${pkg.license}); add its text to scripts/third-party-notices.ts`,
        );
      refs.push(`standard ${pick} text: ${texts.link(standard[pick]!, who)}`);
    }
    return [
      pkg.name,
      pkg.version,
      pkg.license ?? `see ${basename(pkg.license_file!)}`,
      pkg.authors.join(", "),
      pkg.repository ?? pkg.homepage ?? crateUrl(pkg),
      refs.join(", "),
    ];
  });

  const mpl = shipped.filter((p) => {
    const options = licenseOptions(p.license ?? "");
    return options.includes("MPL-2.0") && options.length === 1;
  });

  const grammars = lumisGrammars(metadata, shipped);
  // MPL-2.0 files lumis compiles in (e.g. helix's wat queries): source
  // `url @ rev` → what comes from it.
  const mplSources = new Map<string, string[]>();
  const addMpl = (source: string, what: string) =>
    mplSources.set(source, [...(mplSources.get(source) ?? []), what]);
  for (const g of grammars) {
    if (g.vendored && g.license === "MPL-2.0")
      addMpl(
        g.grammar.replace(/^vendored in lumis \S+ from /, ""),
        `the ${g.language} grammar`,
      );
    if (g.queriesLicense === "MPL-2.0")
      addMpl(g.queries, `the ${g.language} highlight queries`);
  }
  // Vendored grammars and queries ship no license file: standard texts.
  const upstream = new Set(
    grammars.flatMap((g) => [
      ...(g.vendored ? [g.license] : []),
      g.queriesLicense,
    ]),
  );
  const grammarTexts = [...upstream].sort().map((license) => {
    const body = standard[license];
    if (body === undefined)
      throw new Error(
        `no standard ${license} text for the lumis grammars and queries; add one to packaging/license-texts/`,
      );
    return `${license}: ${texts.link(body, "lumis grammars and queries")}`;
  });

  const read = (...p: string[]) =>
    normalize(readFileSync(join(repoRoot, ...p), "utf8"));
  const out: string[] = [];
  const line = (s = "") => out.push(s);

  line("# Third-party notices");
  line();
  line(
    "Polygloss is licensed under MIT OR Apache-2.0. It bundles the third-party software listed here. Generated by `bun scripts/third-party-notices.ts` from `cargo metadata` for the crates linked into `Polygloss` and `polygloss-cli` (aarch64-apple-darwin); do not edit by hand.",
  );
  line();
  line("## Polygloss NOTICE");
  line();
  line(fenced(read("NOTICE")));
  line();
  line("## Lilex font");
  line();
  line(
    "The bundled code font, Lilex (assets/fonts/lilex/), is licensed under the SIL Open Font License, Version 1.1:",
  );
  line();
  line(fenced(read("assets", "fonts", "lilex", "ofl.txt")));
  line();
  line("## Pierre themes");
  line();
  line(
    `Pierre Light and Pierre Dark are ported from @pierre/theme 2.0.0 (Apache-2.0); its NOTICE is quoted in the Polygloss NOTICE above. License text: ${texts.link(standard["Apache-2.0"]!, "Pierre themes")}.`,
  );
  line();
  line("## MPL-2.0 components");
  line();
  if (mpl.length === 0 && mplSources.size === 0) line("None.");
  for (const pkg of mpl)
    line(
      `- ${pkg.name} ${pkg.version} (MPL-2.0) is used unmodified. Its Source Code Form is available at ${crateUrl(pkg)} and ${pkg.repository ?? pkg.homepage ?? crateUrl(pkg)}. The license text is listed with the crate under "Rust crates".`,
    );
  for (const [source, what] of mplSources)
    line(
      `- ${what.join(", ").replace(/^t/, "T")} (MPL-2.0), compiled into Polygloss by lumis, ${what.length === 1 && what[0]!.endsWith("grammar") ? "is" : "are"} used unmodified. The Source Code Form is available at ${source.replace(" @ ", " (revision ")}${source.includes(" @ ") ? ")" : ""}. License text: ${texts.link(standard["MPL-2.0"]!, "lumis grammars and queries")}.`,
    );
  line();
  line("## Syntax grammars and highlight queries");
  line();
  if (grammars.length === 0) {
    line("None.");
  } else {
    const lumis = shipped.find((p) => p.name === "lumis")!;
    line(
      `lumis ${lumis.version} compiles these tree-sitter grammars and highlight queries into Polygloss. Grammars from crates are listed with their license files under "Rust crates"; grammars vendored in lumis and the queries are credited to their upstream repositories. License texts: ${grammarTexts.join(", ")}.`,
    );
    line();
    line(
      "| Language | Grammar | License | Highlight queries | Queries license |",
    );
    line("| --- | --- | --- | --- | --- |");
    for (const g of grammars)
      line(
        `| ${[g.language, g.grammar, g.license, g.queries, g.queriesLicense].map(cell).join(" | ")} |`,
      );
  }
  line();
  line("## Rust crates");
  line();
  line(
    'Each crate\'s license, authors and the license files its published package includes (texts under "License texts").',
  );
  line();
  line("| Crate | Version | License | Authors | Repository | License texts |");
  line("| --- | --- | --- | --- | --- | --- |");
  for (const row of rows)
    line(
      `| ${row
        .map((c, i) => (i === 5 ? c.replace(/\s+/g, " ") : cell(c)))
        .join(" | ")} |`,
    );
  line();
  line("## License texts");
  for (const text of [...texts.byId.values()].sort((a, b) =>
    a.id.localeCompare(b.id),
  )) {
    line();
    line(`### ${text.id}`);
    line();
    line(`Used by: ${text.users.join(", ")}.`);
    line();
    line(fenced(text.body));
  }
  return `${out.join("\n")}\n`;
}

function cargoMetadata(repoRoot: string): Metadata {
  const r = Bun.spawnSync(
    [
      join(repoRoot, "scripts", "cargo.sh"),
      "metadata",
      "--format-version",
      "1",
      "--locked",
      "--filter-platform",
      TARGET,
    ],
    { cwd: repoRoot, stderr: "inherit" },
  );
  if (r.exitCode !== 0)
    throw new Error(`cargo metadata failed with exit code ${r.exitCode}`);
  return JSON.parse(r.stdout.toString()) as Metadata;
}

/**
 * `name version` of every crate `root` links on {@link TARGET}, with its own
 * feature resolution (`cargo tree -p <root> -e normal`).
 */
export function linkedCrates(repoRoot: string, root: string): Set<string> {
  const r = Bun.spawnSync(
    [
      join(repoRoot, "scripts", "cargo.sh"),
      "tree",
      "--locked",
      "-p",
      root,
      "-e",
      "normal",
      "--target",
      TARGET,
      "--prefix",
      "none",
      "--format",
      "{p}",
    ],
    { cwd: repoRoot, stderr: "inherit" },
  );
  if (r.exitCode !== 0)
    throw new Error(
      `cargo tree -p ${root} failed with exit code ${r.exitCode}`,
    );
  const crates = new Set<string>();
  for (const line of r.stdout.toString().split("\n")) {
    const [name, version] = line.trim().split(" ");
    if (name && version) crates.add(`${name} ${version.replace(/^v/, "")}`);
  }
  return crates;
}

function usage(): never {
  console.error(
    "usage: bun scripts/third-party-notices.ts [--check] [--out <file>]",
  );
  process.exit(2);
}

if (import.meta.main) {
  const repoRoot = resolve(import.meta.dir, "..");
  let check = false;
  let out = join(repoRoot, NOTICES_PATH);
  const args = process.argv.slice(2);
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--check") check = true;
    else if (arg === "--out" && args[i + 1]) out = resolve(args[++i]!);
    else usage();
  }
  const linked = new Set(
    SHIPPED_ROOTS.flatMap((root) => [...linkedCrates(repoRoot, root)]),
  );
  const notices = buildNotices({
    metadata: cargoMetadata(repoRoot),
    repoRoot,
    linked,
  });
  const rel = relative(repoRoot, out);
  if (check) {
    const current = existsSync(out) ? readFileSync(out, "utf8") : "";
    if (current !== notices) {
      console.error(
        `third-party-notices: ${rel} is out of date; run bun scripts/third-party-notices.ts and commit it`,
      );
      process.exit(1);
    }
    console.log(`third-party-notices: ${rel} is up to date`);
  } else {
    writeFileSync(out, notices);
    console.log(`third-party-notices: wrote ${rel}`);
  }
}
