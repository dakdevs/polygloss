// The app E2E helpers (tests/support/app.ts) against a stand-in app: a bun
// unix-socket server speaking the §13.3 JSON Lines protocol. No real app.
import { afterAll, describe, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import {
  AppError,
  appCall,
  appWorld,
  stopAllApps,
  stopAppPid,
  waitForApp,
  waitForState,
  waitUntil,
} from "./app";
import { appBin } from "./bins";

const world = appWorld({ session: "support-test" });
afterAll(() => world.cleanup());

type Json = Record<string, any>;

/** A stand-in app answering `hello` (with `pid`), `debug_state` (from
 * `states`, one per call, the last repeated) and failing anything else. */
function fakeApp(opts: { pid: number; states?: Json[]; silent?: boolean }): {
  requests: Json[];
  stop: () => void;
} {
  const requests: Json[] = [];
  let calls = 0;
  const server = Bun.listen({
    unix: world.socket,
    socket: {
      data(socket, data) {
        for (const line of data.toString().split("\n").filter(Boolean)) {
          const req = JSON.parse(line) as Json;
          requests.push(req);
          if (opts.silent) continue;
          const states = opts.states ?? [{}];
          const res =
            req.op === "hello"
              ? {
                  id: req.id,
                  ok: true,
                  result: { app: "polygloss", pid: opts.pid },
                }
              : req.op === "debug_state"
                ? {
                    id: req.id,
                    ok: true,
                    result: states[Math.min(calls++, states.length - 1)],
                  }
                : {
                    id: req.id,
                    ok: false,
                    error: { code: "unknown_op", message: `no ${req.op}` },
                  };
          socket.write(`${JSON.stringify(res)}\n`);
        }
      },
    },
  });
  return { requests, stop: () => server.stop(true) };
}

describe("app E2E helpers", () => {
  test("appWorld is a short test-mode sandbox that launches this checkout's app", () => {
    expect(world.socket.length).toBeLessThan(104);
    expect(
      world.socket.startsWith("/tmp/") ||
        world.socket.startsWith("/private/tmp/"),
    ).toBe(true);
    expect(world.env).toMatchObject({
      POLYGLOSS_DATA_DIR: world.dataDir,
      POLYGLOSS_TEST: "1",
      POLYGLOSS_APP_BIN: appBin(),
      CLAUDE_CODE_SESSION_ID: "support-test",
    });
    expect(world.env.HOME).not.toBe(process.env.HOME);
    expect(world.env.GIT_CONFIG_NOSYSTEM).toBe("1");
  });

  test("appCall sends one versioned JSON line and returns the result", async () => {
    const app = fakeApp({ pid: 4242 });
    try {
      const hello = await appCall(world.socket, "hello", { client: "t" });
      expect(hello).toEqual({ app: "polygloss", pid: 4242 });
      expect(app.requests).toEqual([{ v: 1, id: 1, op: "hello", client: "t" }]);
    } finally {
      app.stop();
    }
  });

  test("appCall rejects app errors with their code and silence with a timeout", async () => {
    const app = fakeApp({ pid: 1 });
    try {
      const err = await appCall(world.socket, "focus").catch((e) => e);
      expect(err).toBeInstanceOf(AppError);
      expect(err.code).toBe("unknown_op");
    } finally {
      app.stop();
    }
    const silent = fakeApp({ pid: 1, silent: true });
    try {
      await expect(appCall(world.socket, "hello", {}, 200)).rejects.toThrow(
        "did not answer hello within 200 ms",
      );
    } finally {
      silent.stop();
    }
  });

  test("waitForState polls debug_state until the check holds", async () => {
    const app = fakeApp({
      pid: 1,
      states: [{ badge: 0 }, { badge: 0 }, { badge: 2 }],
    });
    try {
      const hit = await waitForState(world.socket, (s) =>
        s.badge === 2 ? s : null,
      );
      expect(hit.badge).toBe(2);
      await expect(
        waitForState(world.socket, (s) => s.badge === 3, 300),
      ).rejects.toThrow('"badge": 2');
    } finally {
      app.stop();
    }
  });

  test("waitForApp returns the pid of the app that answers hello", async () => {
    const app = fakeApp({ pid: 99999 });
    try {
      expect(await waitForApp(world.socket, 5_000)).toBe(99999);
    } finally {
      app.stop();
    }
    await expect(waitForApp("/tmp/pge-no-such.sock", 300)).rejects.toThrow(
      "no app answered",
    );
  });

  test("waitUntil fails with what it waited for", async () => {
    let n = 0;
    await waitUntil("three polls", () => ++n >= 3);
    expect(n).toBe(3);
    await expect(waitUntil("never", () => false, 200)).rejects.toThrow(
      "waiting for never",
    );
  });

  test("stopAppPid and stopAllApps stop the processes they know", async () => {
    const sleeper = Bun.spawn(["/bin/sleep", "60"]);
    await stopAppPid(sleeper.pid);
    expect(await sleeper.exited).not.toBe(0);

    // An app found on the socket is stopped too.
    const other = Bun.spawn(["/bin/sleep", "60"]);
    const app = fakeApp({ pid: other.pid });
    try {
      await stopAllApps(world.socket);
    } finally {
      app.stop();
    }
    expect(await other.exited).not.toBe(0);
    expect(existsSync(world.dataDir)).toBe(true);
  });
});
