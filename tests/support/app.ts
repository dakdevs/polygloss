// Drives the real `Polygloss` app in bun E2E suites (plan T4.11): a sandbox
// in test mode (POLYGLOSS_TEST=1, so the socket answers `debug_state`, and
// POLYGLOSS_APP_BIN = this checkout's debug app, so lazy launches start it
// and never the installed bundle), start/stop, and raw socket calls
// (design §13.3: one JSON line per request and per response).
import {
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { createConnection } from "node:net";
import { join } from "node:path";
import { appBin } from "./bins";
import { makeSandbox } from "./sandbox";

type Json = Record<string, any>;
type Env = Record<string, string>;

/** A tab as `debug_state` reports it (crates/polygloss-app/src/ipc/debug_state.rs). */
export type DebugTab = {
  review_id: string;
  diff_id: string;
  title: string;
  active: boolean;
  anchor: { path: string | null; side: string; line: number } | null;
  cursor: { path: string | null; side: string; line: number } | null;
};

/** The app's state as the test-only `debug_state` op reports it. */
export type DebugState = {
  window_open: boolean;
  app_active: boolean;
  tabs: DebugTab[];
  focused_tab: string | null;
  banners: { review_id: string; kind: string; text: string }[];
  badge: number;
  events_seen: number;
  feed_polls: number;
  feed_errors: number;
  /** Times the app asked macOS to activate it (macOS may decline). */
  activations: number;
  /** The Sparkle updater: `"idle"` (loaded in test mode) or none (T5.3). */
  updater: "started" | "idle" | null;
};

/** An error the app answered with (`{ok: false, error: {code, message}}`). */
export class AppError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(`${code}: ${message}`);
  }
}

// Every app process a suite started or found, so a failing test never leaves
// one running (the window would stay on screen).
const running = new Set<number>();

process.on("exit", () => {
  for (const pid of running) {
    try {
      process.kill(pid, "SIGKILL");
    } catch {}
  }
});

/**
 * A sandbox for app E2E tests: the usual throwaway HOME, config and git
 * config, plus a short data dir under /tmp (the socket path must fit in
 * `sun_path`, 104 bytes), test mode, and the debug app as the launch
 * override. `session` becomes `CLAUDE_CODE_SESSION_ID` (the MCP session id).
 */
export function appWorld(opts: { session?: string } = {}): {
  env: Env;
  home: string;
  dataDir: string;
  socket: string;
  cleanup: () => void;
} {
  const sandbox = makeSandbox();
  const dataDir = realpathSync(mkdtempSync("/tmp/pge-"));
  const env: Env = {
    ...sandbox.env,
    POLYGLOSS_DATA_DIR: dataDir,
    POLYGLOSS_TEST: "1",
    POLYGLOSS_APP_BIN: appBin(),
  };
  if (opts.session) env.CLAUDE_CODE_SESSION_ID = opts.session;
  return {
    env,
    home: sandbox.home,
    dataDir,
    socket: join(dataDir, "polygloss.sock"),
    cleanup: () => {
      sandbox.cleanup();
      rmSync(dataDir, { recursive: true, force: true });
    },
  };
}

/** A sandbox made by {@link appWorld}. */
export type AppWorld = ReturnType<typeof appWorld>;

/**
 * One request over the app socket; resolves with `result`, rejects with an
 * {@link AppError} for `ok: false` or an `Error` when the socket cannot be
 * reached or does not answer within `timeoutMs`.
 */
export function appCall(
  socket: string,
  op: string,
  params: Json = {},
  timeoutMs = 10_000,
): Promise<any> {
  return new Promise((resolve, reject) => {
    const conn = createConnection(socket);
    let buffered = "";
    const timer = setTimeout(() => {
      conn.destroy();
      reject(new Error(`app did not answer ${op} within ${timeoutMs} ms`));
    }, timeoutMs);
    const done = (f: () => void) => {
      clearTimeout(timer);
      conn.destroy();
      f();
    };
    conn.on("error", (e) => done(() => reject(e)));
    conn.on("connect", () => {
      conn.write(`${JSON.stringify({ v: 1, id: 1, op, ...params })}\n`);
    });
    conn.on("data", (chunk) => {
      buffered += chunk.toString();
      const nl = buffered.indexOf("\n");
      if (nl < 0) return;
      const res = JSON.parse(buffered.slice(0, nl)) as Json;
      done(() =>
        res.ok
          ? resolve(res.result)
          : reject(new AppError(res.error?.code, res.error?.message)),
      );
    });
  });
}

/** The app's `debug_state`. */
export function debugState(socket: string): Promise<DebugState> {
  return appCall(socket, "debug_state") as Promise<DebugState>;
}

/**
 * Polls `debug_state` until `check` returns a truthy value (which is
 * returned), failing with the last state after `timeoutMs`.
 */
export async function waitForState<T>(
  socket: string,
  check: (s: DebugState) => T | undefined | null | false,
  timeoutMs = 20_000,
): Promise<T> {
  const until = Date.now() + timeoutMs;
  let last: DebugState | undefined;
  for (;;) {
    last = await debugState(socket);
    const hit = check(last);
    if (hit) return hit;
    if (Date.now() > until)
      throw new Error(
        `app state never matched within ${timeoutMs} ms; last: ${JSON.stringify(last, null, 2)}`,
      );
    await Bun.sleep(100);
  }
}

/** Polls `check` every 100 ms until it is true, failing after `timeoutMs`. */
export async function waitUntil(
  what: string,
  check: () => boolean,
  timeoutMs = 20_000,
): Promise<void> {
  const until = Date.now() + timeoutMs;
  while (!check()) {
    if (Date.now() > until)
      throw new Error(`timed out after ${timeoutMs} ms waiting for ${what}`);
    await Bun.sleep(100);
  }
}

/**
 * Waits until an app answers `hello` on `socket`; returns its pid (which
 * is then stopped by {@link stopAppPid} or at exit).
 */
export async function waitForApp(
  socket: string,
  timeoutMs = 60_000,
): Promise<number> {
  const until = Date.now() + timeoutMs;
  let lastError: unknown;
  while (Date.now() < until) {
    if (existsSync(socket)) {
      try {
        const hello = await appCall(
          socket,
          "hello",
          { client: "e2e-tests" },
          2_000,
        );
        running.add(hello.pid as number);
        return hello.pid as number;
      } catch (e) {
        lastError = e;
      }
    }
    await Bun.sleep(100);
  }
  throw new Error(
    `no app answered on ${socket} within ${timeoutMs} ms (${String(lastError)})`,
  );
}

/**
 * Starts `Polygloss [args]` in `world` and waits until it answers on the
 * socket. Its stdout and stderr go to `<dataDir>/app-<n>.log`.
 */
export async function startApp(
  world: { env: Env; dataDir: string; socket: string },
  opts: { args?: string[]; env?: Env; wait?: boolean } = {},
): Promise<{
  proc: Bun.Subprocess;
  pid: number;
  output: () => string;
  stop: () => Promise<void>;
}> {
  const log = join(
    world.dataDir,
    `app-${readdirSync(world.dataDir).filter((f) => f.startsWith("app-")).length}.log`,
  );
  const fd = openSync(log, "a");
  const proc = Bun.spawn([appBin(), ...(opts.args ?? [])], {
    env: { ...world.env, ...opts.env },
    stdin: "ignore",
    stdout: fd,
    stderr: fd,
  });
  running.add(proc.pid);
  const output = () => (existsSync(log) ? readFileSync(log, "utf8") : "");
  if (opts.wait !== false) {
    try {
      await waitForApp(world.socket);
    } catch (e) {
      await stopAppPid(proc.pid);
      throw new Error(`${String(e)}\napp output:\n${output()}`);
    }
  }
  return { proc, pid: proc.pid, output, stop: () => stopAppPid(proc.pid) };
}

/**
 * Stops every app this process started or found, plus any app answering on
 * `socket` (one launched lazily by a CLI or MCP call). Suites call it in
 * `afterAll`, so a failing test leaves no window behind.
 */
export async function stopAllApps(socket?: string): Promise<void> {
  if (socket && existsSync(socket)) {
    try {
      const hello = await appCall(socket, "hello", {}, 2_000);
      running.add(hello.pid as number);
    } catch {}
  }
  await Promise.all([...running].map((pid) => stopAppPid(pid)));
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** Stops the app `pid` (SIGTERM, then SIGKILL after 5 s) and waits for it. */
export async function stopAppPid(pid: number): Promise<void> {
  running.delete(pid);
  if (!alive(pid)) return;
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    return;
  }
  const until = Date.now() + 5_000;
  while (alive(pid) && Date.now() < until) await Bun.sleep(50);
  if (alive(pid)) {
    try {
      process.kill(pid, "SIGKILL");
    } catch {}
    while (alive(pid)) await Bun.sleep(50);
  }
}

/** Runs `git -C repo args` in `env`; returns trimmed stdout. */
export function git(env: Env, repo: string, args: string[]): string {
  const r = Bun.spawnSync(["git", "-C", repo, ...args], { env });
  if (r.exitCode !== 0)
    throw new Error(`git ${args.join(" ")}: ${r.stderr.toString()}`);
  return r.stdout.toString().trim();
}
