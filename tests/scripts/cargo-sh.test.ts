import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
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
const realCargoHome = process.env.CARGO_HOME ?? join(homedir(), ".cargo");
const realRustupHome = process.env.RUSTUP_HOME ?? join(homedir(), ".rustup");

// A fake cargo that reports the environment scripts/cargo.sh hands it.
const fakeCargo = `#!/usr/bin/env bash
for v in CARGO_TARGET_DIR CARGO_BUILD_BUILD_DIR RUSTC RUSTDOC PATH; do
  printf '%s=%s\\n' "$v" "\${!v-}"
done
for a in "$@"; do printf 'ARG=%s\\n' "$a"; done
if [ "\${1-}" = "fail" ]; then exit 3; fi
`;

let root = "";
let fakeCargoHome = "";
let mainCheckout = "";
let worktree = "";
let baseEnv: Record<string, string> = {};

function git(args: string[], cwd: string): string {
  const r = Bun.spawnSync(["git", ...args], { cwd, env: baseEnv });
  if (r.exitCode !== 0)
    throw new Error(`git ${args.join(" ")} failed: ${r.stderr.toString()}`);
  return r.stdout.toString();
}

function runCargoSh(opts: {
  script: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
}): {
  exitCode: number;
  vars: Record<string, string>;
  args: string[];
  stderr: string;
} {
  const r = Bun.spawnSync([opts.script, ...(opts.args ?? [])], {
    cwd: opts.cwd ?? root,
    env: { ...baseEnv, CARGO_HOME: fakeCargoHome, ...opts.env },
  });
  const vars: Record<string, string> = {};
  const args: string[] = [];
  for (const line of r.stdout.toString().split("\n")) {
    if (line.startsWith("ARG=")) args.push(line.slice(4));
    else if (line.includes("="))
      vars[line.slice(0, line.indexOf("="))] = line.slice(
        line.indexOf("=") + 1,
      );
  }
  return {
    exitCode: r.exitCode ?? -1,
    vars,
    args,
    stderr: r.stderr.toString(),
  };
}

beforeAll(() => {
  root = realpathSync(mkdtempSync(join(tmpdir(), "polygloss-cargo-sh-")));
  const home = join(root, "home");
  mkdirSync(home);
  writeFileSync(join(root, "gitconfig"), "");
  baseEnv = {
    HOME: home,
    XDG_CONFIG_HOME: join(home, ".config"),
    GIT_CONFIG_GLOBAL: join(root, "gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_AUTHOR_NAME: "Test",
    GIT_AUTHOR_EMAIL: "test@example.com",
    GIT_COMMITTER_NAME: "Test",
    GIT_COMMITTER_EMAIL: "test@example.com",
    PATH: process.env.PATH ?? "/usr/bin:/bin",
  };

  fakeCargoHome = join(root, "cargo-home");
  mkdirSync(join(fakeCargoHome, "bin"), { recursive: true });
  writeFileSync(join(fakeCargoHome, "bin", "cargo"), fakeCargo);
  chmodSync(join(fakeCargoHome, "bin", "cargo"), 0o755);

  mainCheckout = join(root, "main");
  mkdirSync(join(mainCheckout, "scripts"), { recursive: true });
  copyFileSync(
    join(repoRoot, "scripts", "cargo.sh"),
    join(mainCheckout, "scripts", "cargo.sh"),
  );
  git(["init", "-q", "-b", "main"], mainCheckout);
  git(["add", "-A"], mainCheckout);
  git(["commit", "-q", "-m", "init"], mainCheckout);
  worktree = join(root, "wt with space");
  git(["worktree", "add", "-q", "--detach", worktree], mainCheckout);
});

afterAll(() => {
  if (root) rmSync(root, { recursive: true, force: true });
});

describe("scripts/cargo.sh", () => {
  test("linked worktree gets its own target dir and the main checkout's shared build dir", () => {
    const r = runCargoSh({ script: join(worktree, "scripts", "cargo.sh") });
    expect(r.exitCode).toBe(0);
    expect(r.vars.CARGO_TARGET_DIR).toBe(join(worktree, "target"));
    expect(r.vars.CARGO_BUILD_BUILD_DIR).toBe(
      join(mainCheckout, "target-shared"),
    );
  });

  test("main checkout uses its own target dir and target-shared", () => {
    const r = runCargoSh({
      script: join(mainCheckout, "scripts", "cargo.sh"),
      cwd: worktree,
    });
    expect(r.exitCode).toBe(0);
    expect(r.vars.CARGO_TARGET_DIR).toBe(join(mainCheckout, "target"));
    expect(r.vars.CARGO_BUILD_BUILD_DIR).toBe(
      join(mainCheckout, "target-shared"),
    );
  });

  test("dirs derive from the script's checkout, not the caller's cwd", () => {
    const r = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      cwd: mainCheckout,
    });
    expect(r.vars.CARGO_TARGET_DIR).toBe(join(worktree, "target"));
  });

  test("inherited GIT_DIR and GIT_WORK_TREE are ignored", () => {
    const r = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      env: { GIT_DIR: join(mainCheckout, ".git"), GIT_WORK_TREE: mainCheckout },
    });
    expect(r.exitCode).toBe(0);
    expect(r.vars.CARGO_TARGET_DIR).toBe(join(worktree, "target"));
    expect(r.vars.CARGO_BUILD_BUILD_DIR).toBe(
      join(mainCheckout, "target-shared"),
    );
  });

  test("RUSTC and RUSTDOC point at the rustup proxies and PATH starts with the cargo bin dir", () => {
    const r = runCargoSh({ script: join(worktree, "scripts", "cargo.sh") });
    const bin = join(fakeCargoHome, "bin");
    expect(r.vars.RUSTC).toBe(join(bin, "rustc"));
    expect(r.vars.RUSTDOC).toBe(join(bin, "rustdoc"));
    expect(r.vars.PATH?.split(":")[0]).toBe(bin);
  });

  test("caller-provided values win", () => {
    const r = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      env: {
        CARGO_TARGET_DIR: "/custom/target",
        CARGO_BUILD_BUILD_DIR: "/custom/build",
        RUSTC: "/custom/rustc",
        RUSTDOC: "/custom/rustdoc",
      },
    });
    expect(r.vars.CARGO_TARGET_DIR).toBe("/custom/target");
    expect(r.vars.CARGO_BUILD_BUILD_DIR).toBe("/custom/build");
    expect(r.vars.RUSTC).toBe("/custom/rustc");
    expect(r.vars.RUSTDOC).toBe("/custom/rustdoc");
  });

  test("forwards arguments verbatim and propagates cargo's exit code", () => {
    const ok = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      args: ["build", "-p", "a b", ""],
    });
    expect(ok.args).toEqual(["build", "-p", "a b", ""]);
    const failed = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      args: ["fail"],
    });
    expect(failed.exitCode).toBe(3);
  });

  test("runs rustup's cargo 1.98.1 from the repo", () => {
    const r = Bun.spawnSync(
      [join(repoRoot, "scripts", "cargo.sh"), "--version"],
      {
        cwd: repoRoot,
        env: {
          ...baseEnv,
          CARGO_HOME: realCargoHome,
          RUSTUP_HOME: realRustupHome,
          // Homebrew's cargo first on PATH must not win.
          PATH: `/opt/homebrew/bin:${baseEnv.PATH}`,
        },
      },
    );
    expect(r.stderr.toString()).toBe("");
    expect(r.stdout.toString()).toMatch(/^cargo 1\.98\.1 /);
  });
});
