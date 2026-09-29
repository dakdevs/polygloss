import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");
const checkDeps = join(repoRoot, "scripts", "check-deps.sh");

// Crates whose graphs scripts/check-deps.sh inspects.
const members = [
  "polygloss-diff",
  "polygloss-core",
  "polygloss-highlight",
  "polygloss-viewport",
  "polygloss-platform",
  "polygloss-app",
  "polygloss-mcp",
  "polygloss-cli",
];

// Default fixture wiring mirrors the real dependency direction.
const baseWiring: Record<string, string[]> = {
  "polygloss-core": ["polygloss-diff"],
  "polygloss-viewport": ["polygloss-diff", "polygloss-highlight"],
  "polygloss-app": [
    "polygloss-core",
    "polygloss-diff",
    "polygloss-highlight",
    "polygloss-viewport",
    "polygloss-platform",
  ],
  "polygloss-mcp": ["polygloss-core", "polygloss-platform"],
  "polygloss-cli": ["polygloss-core", "polygloss-mcp", "polygloss-platform"],
};

let root = "";
let env: Record<string, string> = {};
let counter = 0;

beforeAll(() => {
  root = realpathSync(mkdtempSync(join(tmpdir(), "polygloss-check-deps-")));
  const home = join(root, "home");
  mkdirSync(home);
  writeFileSync(join(root, "gitconfig"), "");
  env = {
    HOME: home,
    XDG_CONFIG_HOME: join(home, ".config"),
    GIT_CONFIG_GLOBAL: join(root, "gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
    // Real toolchain, read-only: rustup and cargo live outside the sandboxed HOME.
    CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), ".cargo"),
    RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"),
    PATH: process.env.PATH ?? "/usr/bin:/bin",
  };
});

afterAll(() => {
  if (root) rmSync(root, { recursive: true, force: true });
});

function writeCrate(
  dir: string,
  opts: { name: string; version?: string; deps?: string[]; tail?: string },
): void {
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(join(dir, "src", "lib.rs"), "");
  const deps = (opts.deps ?? []).join("\n");
  writeFileSync(
    join(dir, "Cargo.toml"),
    `[package]\nname = "${opts.name}"\nversion = "${opts.version ?? "0.1.0"}"\nedition = "2024"\n\n[dependencies]\n${deps}\n${opts.tail ?? ""}`,
  );
}

// Builds a path-only fixture workspace. `extra` maps a member to dependency lines;
// `fakes` are stand-in crates (e.g. a fake `gpui-kit`) living under <root>/fakes/<dir>.
function makeWorkspace(opts: {
  extra?: Record<string, string[]>;
  // Raw manifest text appended after a member's [dependencies] table.
  tails?: Record<string, string>;
  fakes?: { dir: string; name: string; version?: string }[];
}): string {
  const ws = join(root, `ws-${counter++}`);
  mkdirSync(ws, { recursive: true });
  copyFileSync(
    join(repoRoot, "rust-toolchain.toml"),
    join(ws, "rust-toolchain.toml"),
  );
  writeFileSync(
    join(ws, "Cargo.toml"),
    `[workspace]\nresolver = "3"\nmembers = ["crates/*"]\n`,
  );
  // Fakes live outside the workspace dir so cargo does not make them members.
  for (const fake of opts.fakes ?? []) {
    writeCrate(join(fakesRoot(), fake.dir), {
      name: fake.name,
      version: fake.version,
    });
  }
  for (const name of members) {
    const deps = (baseWiring[name] ?? []).map(
      (d) => `${d} = { path = "../${d}" }`,
    );
    deps.push(...(opts.extra?.[name] ?? []));
    writeCrate(join(ws, "crates", name), {
      name,
      deps,
      tail: opts.tails?.[name],
    });
  }
  return ws;
}

function fakesRoot(): string {
  return join(root, "fakes");
}

function fake(dir: string): string {
  return `${dir} = { path = "${join(fakesRoot(), dir)}" }`;
}

function run(
  args: string[],
  cwd: string,
): { exitCode: number; output: string } {
  const r = Bun.spawnSync([checkDeps, ...args], { cwd, env });
  return {
    exitCode: r.exitCode ?? -1,
    output: r.stdout.toString() + r.stderr.toString(),
  };
}

function runFixture(ws: string): { exitCode: number; output: string } {
  return run(["--manifest-path", join(ws, "Cargo.toml")], ws);
}

describe("scripts/check-deps.sh", () => {
  test("passes on the real workspace", () => {
    const r = run([], repoRoot);
    expect(r.output).toContain("check-deps: ok");
    expect(r.exitCode).toBe(0);
  }, 120_000);

  test("passes on a clean fixture where only allowed crates reach lumis and tokio", () => {
    const ws = makeWorkspace({
      extra: {
        "polygloss-highlight": [fake("lumis")],
        "polygloss-mcp": [fake("tokio")],
      },
      fakes: [
        { dir: "lumis", name: "lumis" },
        { dir: "tokio", name: "tokio" },
      ],
    });
    const r = runFixture(ws);
    expect(r.output).toContain("check-deps: ok");
    expect(r.exitCode).toBe(0);
  }, 60_000);

  test("fails when polygloss-cli reaches a gpui crate", () => {
    const ws = makeWorkspace({
      extra: { "polygloss-cli": [fake("gpui-kit")] },
      fakes: [{ dir: "gpui-kit", name: "gpui-kit" }],
    });
    const r = runFixture(ws);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toMatch(/polygloss-cli reaches forbidden crate gpui-kit/);
  }, 60_000);

  test("fails when polygloss-cli reaches lumis through another crate", () => {
    const ws = makeWorkspace({
      extra: { "polygloss-mcp": [fake("lumis")] },
      fakes: [{ dir: "lumis", name: "lumis" }],
    });
    const r = runFixture(ws);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toMatch(/polygloss-cli reaches forbidden crate lumis/);
  }, 60_000);

  test("fails when polygloss-highlight reaches a gpui crate", () => {
    const ws = makeWorkspace({
      extra: { "polygloss-highlight": [fake("gpui-pre")] },
      fakes: [{ dir: "gpui-pre", name: "gpui-pre" }],
    });
    const r = runFixture(ws);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toMatch(
      /polygloss-highlight reaches forbidden crate gpui-pre/,
    );
  }, 60_000);

  test("fails when polygloss-core reaches tokio", () => {
    const ws = makeWorkspace({
      extra: { "polygloss-core": [fake("tokio")] },
      fakes: [{ dir: "tokio", name: "tokio" }],
    });
    const r = runFixture(ws);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toMatch(/polygloss-core reaches forbidden crate tokio/);
  }, 60_000);

  // The "Must not contain" column of docs/plan.md's ownership table.
  const forbidden: { crate: string; dep: string }[] = [
    { crate: "polygloss-diff", dep: "lumis" },
    { crate: "polygloss-diff", dep: "rmcp" },
    { crate: "polygloss-diff", dep: "gix" },
    { crate: "polygloss-diff", dep: "rusqlite" },
    { crate: "polygloss-core", dep: "rmcp" },
    { crate: "polygloss-core", dep: "lumis" },
    { crate: "polygloss-highlight", dep: "gix" },
    { crate: "polygloss-highlight", dep: "rusqlite" },
    { crate: "polygloss-viewport", dep: "libsqlite3-sys" },
    { crate: "polygloss-viewport", dep: "gix" },
    { crate: "polygloss-platform", dep: "tokio" },
    { crate: "polygloss-platform", dep: "gpui-kit" },
    { crate: "polygloss-app", dep: "tokio" },
    { crate: "polygloss-app", dep: "rmcp" },
    { crate: "polygloss-mcp", dep: "lumis" },
    { crate: "polygloss-mcp", dep: "gpui-kit" },
  ];
  for (const { crate, dep } of forbidden) {
    test(`fails when ${crate} reaches ${dep}`, () => {
      const dir = `forbidden-${dep}`;
      const ws = makeWorkspace({
        extra: { [crate]: [`${dep} = { path = "${join(fakesRoot(), dir)}" }`] },
        fakes: [{ dir, name: dep }],
      });
      const r = runFixture(ws);
      expect(r.exitCode).toBe(1);
      expect(r.output).toContain(`${crate} reaches forbidden crate ${dep}`);
    }, 60_000);
  }

  test("gix-imara-diff is not mistaken for git access", () => {
    const ws = makeWorkspace({
      extra: { "polygloss-diff": [fake("gix-imara-diff")] },
      fakes: [{ dir: "gix-imara-diff", name: "gix-imara-diff" }],
    });
    const r = runFixture(ws);
    expect(r.output).toContain("check-deps: ok");
    expect(r.exitCode).toBe(0);
  }, 60_000);

  test("passes when a network-capable crate is only in the lockfile for another platform", () => {
    // Like hyper today: in Cargo.lock, but only under cfg(target_family = "wasm"),
    // so `cargo tree -i hyper --target aarch64-apple-darwin` prints nothing.
    const ws = makeWorkspace({
      tails: {
        "polygloss-core": `\n[target.'cfg(target_family = "wasm")'.dependencies]\n${fake("hyper")}\n`,
      },
      fakes: [{ dir: "hyper", name: "hyper" }],
    });
    const r = runFixture(ws);
    expect(r.output).toContain("check-deps: ok");
    expect(r.exitCode).toBe(0);
  }, 60_000);

  test("fails when two libsqlite3-sys versions are in the graph", () => {
    const ws = makeWorkspace({
      extra: {
        "polygloss-core": [
          `sq-a = { package = "libsqlite3-sys", path = "${join(fakesRoot(), "sq-a")}" }`,
          `sq-b = { package = "libsqlite3-sys", path = "${join(fakesRoot(), "sq-b")}" }`,
        ],
      },
      fakes: [
        { dir: "sq-a", name: "libsqlite3-sys", version: "0.35.0" },
        { dir: "sq-b", name: "libsqlite3-sys", version: "0.36.0" },
      ],
    });
    const r = runFixture(ws);
    expect(r.exitCode).toBe(1);
    expect(r.output).toMatch(/more than one libsqlite3-sys/);
  }, 60_000);

  test("fails when two tree-sitter versions are in the graph", () => {
    const ws = makeWorkspace({
      extra: {
        "polygloss-highlight": [
          `ts-a = { package = "tree-sitter", path = "${join(fakesRoot(), "ts-a")}" }`,
          `ts-b = { package = "tree-sitter", path = "${join(fakesRoot(), "ts-b")}" }`,
        ],
      },
      fakes: [
        { dir: "ts-a", name: "tree-sitter", version: "0.25.0" },
        { dir: "ts-b", name: "tree-sitter", version: "0.26.0" },
      ],
    });
    const r = runFixture(ws);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toMatch(/more than one tree-sitter/);
  }, 60_000);

  test("exits 2 instead of passing when cargo cannot read the workspace", () => {
    const ws = makeWorkspace({});
    writeFileSync(
      join(ws, "crates", "polygloss-core", "Cargo.toml"),
      "not toml [",
    );
    const r = runFixture(ws);
    expect(r.exitCode).toBe(2);
    expect(r.output).not.toContain("check-deps: ok");
  }, 60_000);

  test("fails when a network-capable crate is in the graph", () => {
    const ws = makeWorkspace({
      extra: { "polygloss-platform": [fake("ureq")] },
      fakes: [{ dir: "ureq", name: "ureq" }],
    });
    const r = runFixture(ws);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toMatch(/network-capable crate ureq/);
  }, 60_000);
});
