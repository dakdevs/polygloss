// scripts/measure-launch.ts (plan T5.8): cold-launch time of a bundle,
// `open -g` → socket `hello` answered, median of n launches. These tests
// never touch LaunchServices: POLYGLOSS_OPEN replaces `open` with a fake that
// starts a stand-in app (a bun script serving `hello` on the sandbox's
// socket), and POLYGLOSS_LSREGISTER replaces lsregister with a logger.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { median } from "../../scripts/measure-launch";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts/measure-launch.ts");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

let bundle = "";
let fakeOpen = "";
let silentOpen = "";
let fakeLsregister = "";
let log = "";

// The stand-in app: serves `hello` (design §13.3) on
// $POLYGLOSS_DATA_DIR/polygloss.sock after FAKE_APP_DELAY_MS, logs its
// sandbox, and exits on SIGTERM.
const fakeApp = `
import { appendFileSync, unlinkSync } from "node:fs";
import { createServer } from "node:net";
const log = process.env.FAKE_LOG;
appendFileSync(log, "app " + JSON.stringify({ pid: process.pid, home: process.env.HOME, data: process.env.POLYGLOSS_DATA_DIR, test: process.env.POLYGLOSS_TEST }) + "\\n");
const sock = process.env.POLYGLOSS_DATA_DIR + "/polygloss.sock";
process.on("SIGTERM", () => { appendFileSync(log, "term " + process.pid + "\\n"); try { unlinkSync(sock); } catch {} process.exit(0); });
setTimeout(() => {
  createServer((c) => {
    c.on("data", (d) => {
      const req = JSON.parse(d.toString().trim());
      c.write(JSON.stringify({ id: req.id, ok: true, result: { app: "polygloss", version: "0.1.0", pid: process.pid, protocol: 1 } }) + "\\n");
    });
  }).listen(sock);
}, Number(process.env.FAKE_APP_DELAY_MS ?? "0"));
`;

beforeAll(() => {
  const root = sandbox.home;
  log = join(root, "log");
  writeFileSync(log, "");
  bundle = join(root, "Fake.app");
  mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
  writeFileSync(join(bundle, "Contents/MacOS/Polygloss"), "");
  writeFileSync(join(root, "fake-app.ts"), fakeApp);
  // `open -g -F -a <app> --env K=V …`: start the stand-in with exactly the
  // --env values, detached, like LaunchServices would. The script runs `open`
  // in the sandbox's environment only, so the fakes carry their log path.
  fakeOpen = join(root, "fake-open");
  writeFileSync(
    fakeOpen,
    `#!/usr/bin/env bash
FAKE_LOG="${log}"
printf 'open %s\\n' "$*" >>"$FAKE_LOG"
envs=()
while [ $# -gt 0 ]; do
  if [ "$1" = --env ]; then envs+=("$2"); shift; fi
  shift
done
env -i "\${envs[@]}" FAKE_LOG="$FAKE_LOG" FAKE_APP_DELAY_MS="$(cat "${log}.delay")" "${process.execPath}" "${join(root, "fake-app.ts")}" </dev/null >/dev/null 2>&1 &
`,
  );
  silentOpen = join(root, "silent-open");
  writeFileSync(
    silentOpen,
    `#!/usr/bin/env bash\nprintf 'open %s\\n' "$*" >>"${log}"\n`,
  );
  fakeLsregister = join(root, "fake-lsregister");
  writeFileSync(
    fakeLsregister,
    `#!/usr/bin/env bash\nprintf 'lsregister %s\\n' "$*" >>"${log}"\n`,
  );
  for (const f of [fakeOpen, silentOpen, fakeLsregister]) chmodSync(f, 0o755);
});

function measure(
  args: string[],
  opts: { open?: string; delayMs?: number } = {},
): { code: number; stdout: string; stderr: string; log: string } {
  writeFileSync(log, "");
  writeFileSync(`${log}.delay`, String(opts.delayMs ?? 0));
  const r = Bun.spawnSync(["bun", script, ...args], {
    cwd: repoRoot,
    env: {
      ...sandbox.env,
      POLYGLOSS_OPEN: opts.open ?? fakeOpen,
      POLYGLOSS_LSREGISTER: fakeLsregister,
    },
    stdout: "pipe",
    stderr: "pipe",
    timeout: 25_000,
  });
  return {
    code: r.exitCode ?? -1,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
    log: readFileSync(log, "utf8"),
  };
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

describe("median", () => {
  test("median of an odd and an even sample", () => {
    expect(median([5, 1, 3])).toBe(3);
    expect(median([4, 1, 3, 2])).toBe(2.5);
    expect(() => median([])).toThrow();
  });
});

describe("scripts/measure-launch.ts", () => {
  test("times each launch until the app answers hello, then quits it", () => {
    const r = measure([bundle, "--runs", "3", "--json"], { delayMs: 150 });
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
    const out = JSON.parse(r.stdout);
    expect(out.bundle).toBe(bundle);
    expect(out.runs).toHaveLength(3);
    for (const ms of out.runs) {
      expect(ms).toBeGreaterThanOrEqual(150);
      expect(ms).toBeLessThan(10_000);
    }
    expect(out.median_ms).toBe(median(out.runs));
    // Launched in the background, without restoring windows, three times.
    const opens = r.log.split("\n").filter((l) => l.startsWith("open "));
    expect(opens).toHaveLength(3);
    for (const o of opens) expect(o).toStartWith(`open -g -F -a ${bundle} `);
    // Every stand-in was asked to quit and is gone.
    const pids = r.log
      .split("\n")
      .filter((l) => l.startsWith("app "))
      .map((l) => JSON.parse(l.slice(4)).pid as number);
    expect(pids).toHaveLength(3);
    for (const pid of pids) {
      expect(r.log).toContain(`term ${pid}\n`);
      expect(alive(pid)).toBe(false);
    }
    // The bundle is unregistered from LaunchServices once, at the end.
    expect(r.log.trimEnd().split("\n").at(-1)).toBe(`lsregister -u ${bundle}`);
  }, 30_000);

  test("each launch runs in its own sandbox in test mode", () => {
    const r = measure([bundle, "--runs", "2", "--json"]);
    expect(r.code).toBe(0);
    const apps = r.log
      .split("\n")
      .filter((l) => l.startsWith("app "))
      .map((l) => JSON.parse(l.slice(4)));
    expect(apps).toHaveLength(2);
    expect(apps[0].data).not.toBe(apps[1].data);
    for (const a of apps) {
      expect(a.home).not.toBe(homedir());
      expect(
        a.home.startsWith("/tmp/") || a.home.startsWith("/private/tmp/"),
      ).toBe(true);
      expect(a.test).toBe("1");
      // The sandbox is removed afterwards.
      expect(existsSync(a.data)).toBe(false);
    }
  }, 30_000);

  test("human output prints every launch and the median", () => {
    const r = measure([bundle, "--runs", "2"]);
    expect(r.code).toBe(0);
    expect(r.stdout).toMatch(/^launch 1: \d+(\.\d+)? ms$/m);
    expect(r.stdout).toMatch(/^launch 2: \d+(\.\d+)? ms$/m);
    expect(r.stdout).toMatch(/^median of 2: \d+(\.\d+)? ms$/m);
  }, 30_000);

  test("an app that never answers fails the run and still unregisters", () => {
    const r = measure([bundle, "--runs", "2", "--timeout-s", "1"], {
      open: silentOpen,
    });
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("did not answer hello");
    expect(r.log.trimEnd().split("\n").at(-1)).toBe(`lsregister -u ${bundle}`);
  }, 30_000);

  test("usage errors exit 2", () => {
    expect(measure([]).code).toBe(2);
    expect(measure([join(sandbox.home, "missing.app")]).code).toBe(2);
    expect(measure([bundle, "--runs", "0"]).code).toBe(2);
  });
});
