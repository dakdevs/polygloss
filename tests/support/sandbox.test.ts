import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, relative } from "node:path";
import { makeSandbox } from "./sandbox";

const cleanups: Array<() => void> = [];
afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()?.();
});

function sandbox(): ReturnType<typeof makeSandbox> {
  const s = makeSandbox();
  cleanups.push(s.cleanup);
  return s;
}

function isInside(child: string, parent: string): boolean {
  const rel = relative(parent, child);
  return rel !== "" && !rel.startsWith("..") && !rel.startsWith("/");
}

describe("makeSandbox", () => {
  test("sandbox never points at the real HOME", () => {
    const s = sandbox();
    const realHome = realpathSync(homedir());
    const tmp = realpathSync(tmpdir());
    const paths = [
      s.home,
      s.dataDir,
      s.configDir,
      s.env.HOME,
      s.env.POLYGLOSS_DATA_DIR,
      s.env.XDG_CONFIG_HOME,
      s.env.XDG_CACHE_HOME,
      s.env.GIT_CONFIG_GLOBAL,
    ];
    for (const p of paths) {
      expect(p).toBeString();
      const path = p as string;
      expect(isInside(path, tmp)).toBe(true);
      expect(path === realHome || isInside(path, realHome)).toBe(false);
    }
    expect(s.env.HOME).toBe(s.home);
    expect(s.env.POLYGLOSS_DATA_DIR).toBe(s.dataDir);
    expect(s.env.XDG_CONFIG_HOME).toBe(s.configDir);
  });

  test("cache dir lives under the sandbox HOME and every dir exists", () => {
    const s = sandbox();
    expect(isInside(s.env.XDG_CACHE_HOME ?? "", s.home)).toBe(true);
    for (const dir of [s.home, s.dataDir, s.configDir, s.env.XDG_CACHE_HOME])
      expect(existsSync(dir ?? "")).toBe(true);
  });

  test("git config is an empty global file with system config disabled", () => {
    const s = sandbox();
    expect(readFileSync(s.env.GIT_CONFIG_GLOBAL ?? "", "utf8")).toBe("");
    expect(s.env.GIT_CONFIG_NOSYSTEM).toBe("1");
  });

  test("git identity and dates are fixed", () => {
    const s = sandbox();
    for (const role of ["AUTHOR", "COMMITTER"]) {
      expect(s.env[`GIT_${role}_NAME`]).toBeTruthy();
      expect(s.env[`GIT_${role}_EMAIL`]).toBeTruthy();
      expect(s.env[`GIT_${role}_DATE`]).toBeTruthy();
    }
  });

  test("inherited git repository overrides are not passed through", () => {
    const saved = process.env.GIT_DIR;
    process.env.GIT_DIR = "/somewhere/else/.git";
    try {
      const s = sandbox();
      expect(s.env.GIT_DIR).toBeUndefined();
      expect(s.env.PATH).toBe(process.env.PATH);
    } finally {
      if (saved === undefined) delete process.env.GIT_DIR;
      else process.env.GIT_DIR = saved;
    }
  });

  test("a child process sees the sandbox HOME", () => {
    const s = sandbox();
    const r = Bun.spawnSync(["/bin/sh", "-c", 'printf %s "$HOME"'], {
      env: s.env,
    });
    expect(r.stdout.toString()).toBe(s.home);
  });

  test("two sandboxes are distinct", () => {
    expect(sandbox().home).not.toBe(sandbox().home);
  });

  test("cleanup removes the sandbox", () => {
    const s = makeSandbox();
    s.cleanup();
    expect(existsSync(s.home)).toBe(false);
    expect(existsSync(s.dataDir)).toBe(false);
  });
});
