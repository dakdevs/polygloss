import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");
const realCargoHome = process.env.CARGO_HOME ?? join(homedir(), ".cargo");
const realRustupHome = process.env.RUSTUP_HOME ?? join(homedir(), ".rustup");

// A fake cargo that reports the environment scripts/cargo.sh hands it and, when
// FAKE_CARGO_LOG is set, appends every invocation (including cargo.sh's own
// `clean` calls, whose stdout goes to stderr) to that file.
const fakeCargo = `#!/usr/bin/env bash
if [ -n "\${FAKE_CARGO_LOG-}" ]; then printf 'cargo %s\\n' "$*" >>"$FAKE_CARGO_LOG"; fi
case "\${1-}" in
  fail) exit 3 ;;
  sleep-log)
    printf 'start %s\\n' "$2" >>"$FAKE_CARGO_LOG"
    sleep 1
    printf 'end %s\\n' "$2" >>"$FAKE_CARGO_LOG"
    exit 0 ;;
  nested) shift; exec "$@" ;;
esac
for v in CARGO_TARGET_DIR CARGO_BUILD_BUILD_DIR RUSTC RUSTDOC PATH; do
  printf '%s=%s\\n' "$v" "\${!v-}"
done
for a in "$@"; do printf 'ARG=%s\\n' "$a"; done
`;

let root = "";
let fakeCargoHome = "";
let mainCheckout = "";
let worktree = "";
let baseEnv: Record<string, string> = {};
let counter = 0;

function git(args: string[], cwd: string): string {
  const r = Bun.spawnSync(["git", ...args], { cwd, env: baseEnv });
  if (r.exitCode !== 0)
    throw new Error(`git ${args.join(" ")} failed: ${r.stderr.toString()}`);
  return r.stdout.toString();
}

function parseFakeOutput(stdout: string): {
  vars: Record<string, string>;
  args: string[];
} {
  const vars: Record<string, string> = {};
  const args: string[] = [];
  for (const line of stdout.split("\n")) {
    if (line.startsWith("ARG=")) args.push(line.slice(4));
    else if (line.includes("="))
      vars[line.slice(0, line.indexOf("="))] = line.slice(
        line.indexOf("=") + 1,
      );
  }
  return { vars, args };
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
  return {
    exitCode: r.exitCode ?? -1,
    ...parseFakeOutput(r.stdout.toString()),
    stderr: r.stderr.toString(),
  };
}

// A fresh build dir and invocation log, so marker and lock tests do not interact.
function isolated(): { buildDir: string; log: string } {
  const dir = join(root, `iso-${counter++}`);
  mkdirSync(dir);
  return { buildDir: join(dir, "build"), log: join(dir, "cargo.log") };
}

function logLines(log: string): string[] {
  return existsSync(log)
    ? readFileSync(log, "utf8").split("\n").filter(Boolean)
    : [];
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
  // Content is irrelevant to the fake cargo; cargo.sh only needs a workspace to clean.
  writeFileSync(join(mainCheckout, "Cargo.toml"), "[workspace]\n");
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
    const { buildDir } = isolated();
    const r = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      env: {
        CARGO_TARGET_DIR: "/custom/target",
        CARGO_BUILD_BUILD_DIR: buildDir,
        RUSTC: "/custom/rustc",
        RUSTDOC: "/custom/rustdoc",
      },
    });
    expect(r.exitCode).toBe(0);
    expect(r.vars.CARGO_TARGET_DIR).toBe("/custom/target");
    expect(r.vars.CARGO_BUILD_BUILD_DIR).toBe(buildDir);
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

  test("outside any git checkout, both dirs live in the script's own checkout", () => {
    const tarball = join(root, "tarball");
    mkdirSync(join(tarball, "scripts"), { recursive: true });
    copyFileSync(
      join(repoRoot, "scripts", "cargo.sh"),
      join(tarball, "scripts", "cargo.sh"),
    );
    const r = runCargoSh({ script: join(tarball, "scripts", "cargo.sh") });
    expect(r.exitCode).toBe(0);
    expect(r.vars.CARGO_TARGET_DIR).toBe(join(tarball, "target"));
    expect(r.vars.CARGO_BUILD_BUILD_DIR).toBe(join(tarball, "target-shared"));
  });

  test("fails loudly instead of falling back when git fails inside a checkout", () => {
    // A cargo home whose bin dir also shadows git with one that always fails,
    // as `git` does on a safe.directory "dubious ownership" error.
    const brokenHome = join(root, "broken-git-home");
    mkdirSync(join(brokenHome, "bin"), { recursive: true });
    writeFileSync(join(brokenHome, "bin", "cargo"), fakeCargo);
    chmodSync(join(brokenHome, "bin", "cargo"), 0o755);
    writeFileSync(
      join(brokenHome, "bin", "git"),
      "#!/bin/sh\necho 'fatal: detected dubious ownership in repository' >&2\nexit 128\n",
    );
    chmodSync(join(brokenHome, "bin", "git"), 0o755);
    const r = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      env: { CARGO_HOME: brokenHome },
    });
    expect(r.exitCode).not.toBe(0);
    expect(r.stderr).toContain("dubious ownership");
    expect(r.vars.CARGO_TARGET_DIR).toBeUndefined();
    expect(existsSync(join(worktree, "target-shared"))).toBe(false);
  });

  test("a checkout taking over a build dir another checkout used cleans its workspace members first", () => {
    const { buildDir, log } = isolated();
    const env = { CARGO_BUILD_BUILD_DIR: buildDir, FAKE_CARGO_LOG: log };
    const fromMain = runCargoSh({
      script: join(mainCheckout, "scripts", "cargo.sh"),
      args: ["build"],
      env,
    });
    expect(fromMain.exitCode).toBe(0);
    for (const dir of [
      "debug",
      "release",
      "perf",
      join("aarch64-apple-darwin", "debug"),
    ])
      mkdirSync(join(buildDir, dir, ".fingerprint"), { recursive: true });
    writeFileSync(log, "");

    const fromWorktree = runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      args: ["build"],
      env,
    });
    expect(fromWorktree.exitCode).toBe(0);
    expect(fromWorktree.args).toEqual(["build"]);
    const manifest = join(worktree, "Cargo.toml");
    const lines = logLines(log);
    expect(lines.slice(0, -1).sort()).toEqual(
      [
        `cargo clean -q --workspace --manifest-path ${manifest} --profile dev`,
        `cargo clean -q --workspace --manifest-path ${manifest} --profile release`,
        `cargo clean -q --workspace --manifest-path ${manifest} --profile perf`,
        `cargo clean -q --workspace --manifest-path ${manifest} --profile dev --target aarch64-apple-darwin`,
      ].sort(),
    );
    expect(lines.at(-1)).toBe("cargo build");

    // Same checkout again: nothing to clean.
    writeFileSync(log, "");
    runCargoSh({
      script: join(worktree, "scripts", "cargo.sh"),
      args: ["build"],
      env,
    });
    expect(logLines(log)).toEqual(["cargo build"]);
  });

  test("commands that never touch the build dir neither clean nor take the lock", () => {
    const { buildDir, log } = isolated();
    const env = { CARGO_BUILD_BUILD_DIR: buildDir, FAKE_CARGO_LOG: log };
    runCargoSh({
      script: join(mainCheckout, "scripts", "cargo.sh"),
      args: ["build"],
      env,
    });
    mkdirSync(join(buildDir, "debug", ".fingerprint"), { recursive: true });
    writeFileSync(log, "");
    // Someone else holds the build dir's lock for the whole test, and marks
    // when it lets go: a command that waited for the lock finishes after that.
    const released = `${buildDir}.released`;
    const holder = Bun.spawn(
      [
        "/usr/bin/lockf",
        "-k",
        `${buildDir}.lock`,
        "/bin/sh",
        "-c",
        `sleep 10; touch '${released}'`,
      ],
      { stdout: "ignore", stderr: "ignore" },
    );
    try {
      Bun.sleepSync(200);
      for (const args of [["tree", "-p", "x"], ["metadata"], ["--version"]]) {
        const r = runCargoSh({
          script: join(worktree, "scripts", "cargo.sh"),
          args,
          env,
        });
        expect(r.exitCode).toBe(0);
      }
      expect(existsSync(released)).toBe(false);
    } finally {
      holder.kill();
    }
    expect(logLines(log)).toEqual([
      "cargo tree -p x",
      "cargo metadata",
      "cargo --version",
    ]);
  }, 20_000);

  test("invocations from two checkouts sharing a build dir never overlap", async () => {
    const { buildDir, log } = isolated();
    const env = {
      ...baseEnv,
      CARGO_HOME: fakeCargoHome,
      CARGO_BUILD_BUILD_DIR: buildDir,
      FAKE_CARGO_LOG: log,
    };
    const a = Bun.spawn(
      [join(mainCheckout, "scripts", "cargo.sh"), "sleep-log", "a"],
      { cwd: root, env, stdout: "ignore", stderr: "ignore" },
    );
    await Bun.sleep(200);
    const b = Bun.spawn(
      [join(worktree, "scripts", "cargo.sh"), "sleep-log", "b"],
      { cwd: root, env, stdout: "ignore", stderr: "ignore" },
    );
    expect(await a.exited).toBe(0);
    expect(await b.exited).toBe(0);
    const events = logLines(log).filter((l) => !l.startsWith("cargo "));
    expect(events).toEqual(["start a", "end a", "start b", "end b"]);
  }, 20_000);

  test("a nested call from the same checkout reuses the held lock instead of deadlocking", () => {
    const { buildDir } = isolated();
    const script = join(worktree, "scripts", "cargo.sh");
    const r = Bun.spawnSync([script, "nested", script, "build"], {
      cwd: root,
      env: {
        ...baseEnv,
        CARGO_HOME: fakeCargoHome,
        CARGO_BUILD_BUILD_DIR: buildDir,
      },
      timeout: 10_000,
    });
    expect(r.exitCode).toBe(0);
    expect(parseFakeOutput(r.stdout.toString()).args).toEqual(["build"]);
  });

  test("with-lock runs a command under the lock, never cargo, and its cargo calls reuse the lock", () => {
    const { buildDir, log } = isolated();
    const script = join(worktree, "scripts", "cargo.sh");
    const r = Bun.spawnSync([script, "with-lock", script, "build"], {
      cwd: root,
      env: {
        ...baseEnv,
        CARGO_HOME: fakeCargoHome,
        CARGO_BUILD_BUILD_DIR: buildDir,
        FAKE_CARGO_LOG: log,
      },
      timeout: 10_000,
    });
    expect(r.stderr.toString()).toBe("");
    expect(r.exitCode).toBe(0);
    expect(parseFakeOutput(r.stdout.toString()).args).toEqual(["build"]);
    expect(logLines(log)).toEqual(["cargo build"]);
  });

  test("with-lock holds the lock for the whole command, so other checkouts wait", async () => {
    const { buildDir, log } = isolated();
    const env = {
      ...baseEnv,
      CARGO_HOME: fakeCargoHome,
      CARGO_BUILD_BUILD_DIR: buildDir,
      FAKE_CARGO_LOG: log,
    };
    const a = Bun.spawn(
      [
        join(mainCheckout, "scripts", "cargo.sh"),
        "with-lock",
        "/bin/sh",
        "-c",
        `echo 'start a' >>"$FAKE_CARGO_LOG"; sleep 1; echo 'end a' >>"$FAKE_CARGO_LOG"`,
      ],
      { cwd: root, env, stdout: "ignore", stderr: "ignore" },
    );
    await Bun.sleep(200);
    const b = Bun.spawn(
      [join(worktree, "scripts", "cargo.sh"), "sleep-log", "b"],
      { cwd: root, env, stdout: "ignore", stderr: "ignore" },
    );
    expect(await a.exited).toBe(0);
    expect(await b.exited).toBe(0);
    const events = logLines(log).filter((l) => !l.startsWith("cargo "));
    expect(events).toEqual(["start a", "end a", "start b", "end b"]);
  }, 20_000);

  test("a nested call from another checkout sharing the held build dir fails loudly", () => {
    const { buildDir } = isolated();
    const r = Bun.spawnSync(
      [
        join(worktree, "scripts", "cargo.sh"),
        "nested",
        join(mainCheckout, "scripts", "cargo.sh"),
        "build",
      ],
      {
        cwd: root,
        env: {
          ...baseEnv,
          CARGO_HOME: fakeCargoHome,
          CARGO_BUILD_BUILD_DIR: buildDir,
        },
        timeout: 10_000,
      },
    );
    expect(r.exitCode).not.toBe(0);
    expect(r.stderr.toString()).toContain("another checkout");
    expect(parseFakeOutput(r.stdout.toString()).args).toEqual([]);
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

// Real cargo: two checkouts with diverging sources share one build dir. Cargo names
// workspace-member outputs by the package path relative to the workspace root, so
// without cargo.sh's takeover clean the second checkout would reuse the first
// checkout's artifacts whenever its sources are older than them.
describe("scripts/cargo.sh with real cargo and a shared build dir", () => {
  let repo = "";
  let other = "";
  let env: Record<string, string> = {};

  function writeBin(checkout: string, text: string, mtime: Date): void {
    const file = join(checkout, "crates", "tiny", "src", "main.rs");
    writeFileSync(file, `fn main() {\n    println!("${text}");\n}\n`);
    utimesSync(file, mtime, mtime);
  }

  function cargoRun(checkout: string): { exitCode: number; out: string } {
    const r = Bun.spawnSync(
      [join(checkout, "scripts", "cargo.sh"), "run", "-q", "--offline"],
      { cwd: checkout, env },
    );
    return {
      exitCode: r.exitCode ?? -1,
      out: r.stdout.toString().trim() + r.stderr.toString(),
    };
  }

  beforeAll(() => {
    repo = join(root, "real-main");
    mkdirSync(join(repo, "scripts"), { recursive: true });
    mkdirSync(join(repo, "crates", "tiny", "src"), { recursive: true });
    copyFileSync(
      join(repoRoot, "scripts", "cargo.sh"),
      join(repo, "scripts", "cargo.sh"),
    );
    copyFileSync(
      join(repoRoot, "rust-toolchain.toml"),
      join(repo, "rust-toolchain.toml"),
    );
    writeFileSync(
      join(repo, "Cargo.toml"),
      '[workspace]\nresolver = "3"\nmembers = ["crates/tiny"]\n',
    );
    writeFileSync(
      join(repo, "crates", "tiny", "Cargo.toml"),
      '[package]\nname = "tiny"\nversion = "0.1.0"\nedition = "2024"\n',
    );
    writeFileSync(join(repo, ".gitignore"), "/target\n/target-shared\n");
    writeBin(repo, "main", new Date());
    git(["init", "-q", "-b", "main"], repo);
    git(["add", "-A"], repo);
    git(["commit", "-q", "-m", "init"], repo);
    other = join(root, "real-worktree");
    git(["worktree", "add", "-q", "--detach", other], repo);
    env = {
      ...baseEnv,
      CARGO_HOME: realCargoHome,
      RUSTUP_HOME: realRustupHome,
    };
  });

  test("each checkout builds and runs its own sources", () => {
    expect(cargoRun(repo)).toEqual({ exitCode: 0, out: "main" });
    // Older than the artifacts main just wrote: mtime alone says "fresh".
    writeBin(other, "worktree", new Date("2001-01-01T00:00:00Z"));
    expect(cargoRun(other)).toEqual({ exitCode: 0, out: "worktree" });
    // And back again, with main's source older than the worktree's artifacts.
    utimesSync(
      join(repo, "crates", "tiny", "src", "main.rs"),
      new Date("2001-01-01T00:00:00Z"),
      new Date("2001-01-01T00:00:00Z"),
    );
    expect(cargoRun(repo)).toEqual({ exitCode: 0, out: "main" });
    // Final artifacts stay per checkout.
    expect(
      Bun.spawnSync([join(repo, "target", "debug", "tiny")]).stdout.toString(),
    ).toBe("main\n");
    expect(
      Bun.spawnSync([join(other, "target", "debug", "tiny")]).stdout.toString(),
    ).toBe("worktree\n");
    expect(existsSync(join(other, "target-shared"))).toBe(false);
  }, 120_000);
});
