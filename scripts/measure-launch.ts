// Cold-launch time of a packaged Polygloss.app (plan T5.8, OQ-P12):
//
//   bun scripts/measure-launch.ts <path/to/Polygloss.app> [--runs 5]
//     [--timeout-s 60] [--json]
//
// Each run launches the bundle from where it is (never /Applications) through
// LaunchServices in the background, `open -g -F -a <app>`, in a fresh sandbox
// (a throwaway HOME, data dir, cache, logs, config and git config under /tmp,
// so the socket path fits in sun_path; POLYGLOSS_TEST=1 as in
// scripts/smoke-bundle.sh, so the app posts no notifications), and measures
// from spawning `open` to the app answering `hello` on its socket (design
// §13.3). It then quits the app (SIGTERM, SIGKILL after 10 s) and removes the
// sandbox. Every launch is a new process, so each pays gpui-kit's runtime
// shader compilation (OQ-P12); the result is reported, not budgeted. Prints
// every launch and the median (or, with --json, `{bundle, runs, median_ms}`).
// At the end, pass or fail, the bundle is unregistered from LaunchServices
// again (`lsregister -u`), like smoke-bundle.sh does.
//
// Exit codes: 0 measured, 1 a launch failed, 2 usage.
// Test seams: POLYGLOSS_OPEN (default /usr/bin/open) and POLYGLOSS_LSREGISTER.
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { connect } from "node:net";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const LSREGISTER =
  "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/** The median of a non-empty sample (the mean of the middle two for an even count). */
export function median(values: number[]): number {
  if (values.length === 0) throw new Error("median of an empty sample");
  const s = [...values].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 === 1 ? s[mid]! : (s[mid - 1]! + s[mid]!) / 2;
}

class UsageError extends Error {}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** One `hello` over the app socket; resolves to its result, or null when nothing answers. */
function hello(socket: string): Promise<{ pid: number } | null> {
  return new Promise((done) => {
    const c = connect(socket);
    let buf = "";
    const finish = (v: { pid: number } | null) => {
      c.destroy();
      done(v);
    };
    const timer = setTimeout(() => finish(null), 2_000);
    c.on("error", () => {
      clearTimeout(timer);
      finish(null);
    });
    c.on("connect", () =>
      c.write(
        JSON.stringify({ v: 1, id: 1, op: "hello", client: "measure-launch" }) +
          "\n",
      ),
    );
    c.on("data", (d) => {
      buf += d.toString();
      const nl = buf.indexOf("\n");
      if (nl < 0) return;
      clearTimeout(timer);
      try {
        const res = JSON.parse(buf.slice(0, nl));
        finish(
          res.ok && typeof res.result?.pid === "number" ? res.result : null,
        );
      } catch {
        finish(null);
      }
    });
  });
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function quit(pid: number): Promise<void> {
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    return;
  }
  for (let i = 0; i < 100 && alive(pid); i++) await sleep(100);
  if (alive(pid)) {
    try {
      process.kill(pid, "SIGKILL");
    } catch {}
  }
}

/** A sandbox under /tmp (short, so `<data>/polygloss.sock` fits in sun_path). */
function makeLaunchSandbox(): { root: string; env: Record<string, string> } {
  const root = mkdtempSync("/tmp/pglaunch-");
  const home = join(root, "home");
  const data = join(root, "data");
  for (const d of [join(home, ".config"), join(home, ".cache"), data])
    mkdirSync(d, { recursive: true, mode: 0o700 });
  writeFileSync(join(root, "gitconfig"), "");
  const env: Record<string, string> = {
    HOME: home,
    POLYGLOSS_DATA_DIR: data,
    POLYGLOSS_CACHE_DIR: join(root, "cache"),
    POLYGLOSS_LOG_DIR: join(root, "logs"),
    XDG_CONFIG_HOME: join(home, ".config"),
    XDG_CACHE_HOME: join(home, ".cache"),
    GIT_CONFIG_GLOBAL: join(root, "gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
    POLYGLOSS_TEST: "1",
    PATH: `${process.env.PATH ?? ""}:/usr/bin:/bin:/usr/sbin:/sbin`,
    TMPDIR: process.env.TMPDIR ?? "/tmp",
    LANG: process.env.LANG ?? "en_US.UTF-8",
  };
  return { root, env };
}

/** One launch: ms from spawning `open` to `hello` answered. */
async function launchOnce(app: string, timeoutMs: number): Promise<number> {
  const { root, env } = makeLaunchSandbox();
  const socket = join(env.POLYGLOSS_DATA_DIR!, "polygloss.sock");
  const openBin = process.env.POLYGLOSS_OPEN || "/usr/bin/open";
  const envArgs = Object.entries(env).flatMap(([k, v]) => [
    "--env",
    `${k}=${v}`,
  ]);
  let pid: number | null = null;
  try {
    const start = performance.now();
    const opened = Bun.spawn([openBin, "-g", "-F", "-a", app, ...envArgs], {
      env,
      stdout: "ignore",
      stderr: "pipe",
    });
    const deadline = start + timeoutMs;
    while (performance.now() < deadline) {
      if (existsSync(socket)) {
        const r = await hello(socket);
        if (r) {
          const ms = performance.now() - start;
          pid = r.pid;
          return Math.round(ms * 10) / 10;
        }
      }
      await sleep(10);
    }
    await opened.exited;
    const err = (await new Response(opened.stderr).text()).trim();
    throw new Error(
      `the app did not answer hello on ${socket} within ${timeoutMs / 1000} s` +
        (opened.exitCode ? ` (open exited ${opened.exitCode}: ${err})` : ""),
    );
  } finally {
    if (pid !== null) await quit(pid);
    rmSync(root, { recursive: true, force: true });
  }
}

async function main(argv: string[]): Promise<number> {
  let app: string;
  let runs: number;
  let timeoutMs: number;
  let json: boolean;
  try {
    const { values, positionals } = parseArgs({
      args: argv,
      allowPositionals: true,
      options: {
        runs: { type: "string", default: "5" },
        "timeout-s": { type: "string", default: "60" },
        json: { type: "boolean", default: false },
      },
    });
    if (positionals.length !== 1)
      throw new UsageError("expected one bundle path");
    app = resolve(positionals[0]!);
    if (!existsSync(join(app, "Contents/MacOS/Polygloss")))
      throw new UsageError(`${app} has no Contents/MacOS/Polygloss`);
    runs = Number(values.runs);
    timeoutMs = Number(values["timeout-s"]) * 1000;
    if (!Number.isInteger(runs) || runs < 1)
      throw new UsageError("--runs must be ≥ 1");
    if (!(timeoutMs > 0)) throw new UsageError("--timeout-s must be > 0");
    json = values.json;
  } catch (e) {
    console.error(
      `measure-launch: ${(e as Error).message}\nusage: bun scripts/measure-launch.ts <path/to/Polygloss.app> [--runs 5] [--timeout-s 60] [--json]`,
    );
    return 2;
  }

  const exe = join(app, "Contents/MacOS/Polygloss");
  const running = Bun.spawnSync(["pgrep", "-f", `^${exe}`], { stdout: "pipe" });
  if (running.exitCode === 0) {
    console.error(`measure-launch: ${exe} is already running; quit it first`);
    return 1;
  }

  const results: number[] = [];
  try {
    for (let i = 1; i <= runs; i++) {
      const ms = await launchOnce(app, timeoutMs);
      results.push(ms);
      if (!json) console.log(`launch ${i}: ${ms} ms`);
    }
  } catch (e) {
    console.error(`measure-launch: ${(e as Error).message}`);
    return 1;
  } finally {
    Bun.spawnSync([process.env.POLYGLOSS_LSREGISTER || LSREGISTER, "-u", app], {
      stdout: "ignore",
      stderr: "ignore",
    });
  }
  const med = median(results);
  if (json)
    console.log(JSON.stringify({ bundle: app, runs: results, median_ms: med }));
  else console.log(`median of ${runs}: ${med} ms`);
  return 0;
}

if (import.meta.main) process.exit(await main(process.argv.slice(2)));
