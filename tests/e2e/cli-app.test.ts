// Human entry points against the real running app (plan T4.11, design §13.4,
// §14): a second `Polygloss` hands its argv (a review or a `polygloss://`
// URL) to the running one and exits, and `polygloss open` shows a diff and
// asks for the app to come forward. The app runs in a sandbox with POLYGLOSS_TEST=1
// and is inspected through `debug_state`. Windows appear on screen.
import {
  afterAll,
  beforeAll,
  describe,
  expect,
  setDefaultTimeout,
  test,
} from "bun:test";
import { mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  type AppWorld,
  appWorld,
  debugState,
  git,
  startApp,
  stopAllApps,
  waitForApp,
  waitForState,
} from "../support/app";
import { appBin, cliBin } from "../support/bins";

setDefaultTimeout(120_000);

type Json = Record<string, any>;
type Env = Record<string, string>;

let repoCount = 0;

/** A repo with two commits on `main` and an uncommitted edit of `a.txt`
 * (unique per repo, so no two repos share a diff id). */
function makeRepo(env: Env, home: string): string {
  repoCount += 1;
  const repo = join(home, `cli-repo-${repoCount}`);
  mkdirSync(repo, { recursive: true });
  git(env, repo, ["init", "-q", "-b", "main"]);
  writeFileSync(join(repo, "a.txt"), "one\ntwo\nthree\n");
  git(env, repo, ["add", "."]);
  git(env, repo, ["commit", "-q", "-m", "init"]);
  writeFileSync(join(repo, "b.txt"), `b of repo ${repoCount}\n`);
  git(env, repo, ["add", "."]);
  git(env, repo, ["commit", "-q", "-m", "add b"]);
  writeFileSync(join(repo, "a.txt"), `one\nTWO ${repoCount}\nthree\n`);
  return realpathSync(repo);
}

/** Runs `polygloss-cli args` (JSON mode: stdout is a pipe). */
function cli(
  env: Env,
  args: string[],
  cwd?: string,
): { exitCode: number; json: Json; stderr: string } {
  const r = Bun.spawnSync([cliBin(), ...args], { env, cwd });
  const stdout = r.stdout.toString();
  return {
    exitCode: r.exitCode ?? -1,
    json: stdout.trim() === "" ? {} : (JSON.parse(stdout) as Json),
    stderr: r.stderr.toString(),
  };
}

/** Runs a second `Polygloss args` to its exit (at most 60 s). */
async function secondInstance(
  env: Env,
  args: string[],
): Promise<{ exitCode: number | null; stderr: string; ms: number }> {
  const started = performance.now();
  const proc = Bun.spawn([appBin(), ...args], {
    env,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const exited = await Promise.race([
    proc.exited,
    Bun.sleep(60_000).then(() => "timeout" as const),
  ]);
  if (exited === "timeout") {
    proc.kill("SIGKILL");
    await proc.exited;
  }
  return {
    exitCode: exited === "timeout" ? null : exited,
    stderr: await new Response(proc.stderr).text(),
    ms: performance.now() - started,
  };
}

describe.skipIf(!process.env.POLYGLOSS_E2E)(
  "human commands against a running app",
  () => {
    // Made in beforeAll: a skipped describe runs its body but no hooks.
    let world: AppWorld;
    let appPid = 0;

    beforeAll(async () => {
      world = appWorld();
      appPid = (await startApp(world)).pid;
    });

    afterAll(async () => {
      await stopAllApps(world.socket);
      world.cleanup();
    });

    test("second app instance forwards argv and exits", async () => {
      const repo = makeRepo(world.env, world.home);
      const before = await debugState(world.socket);

      const second = await secondInstance(world.env, [
        "--repo",
        repo,
        "--live",
      ]);
      expect(second.stderr).toContain("already running");
      expect(second.exitCode).toBe(0);
      // The running app (same pid) shows the review the argv named.
      expect(await waitForApp(world.socket)).toBe(appPid);
      const state = await waitForState(world.socket, (s) =>
        s.tabs.length === before.tabs.length + 1 ? s : null,
      );
      const tab = state.tabs.find((t) => t.active)!;
      expect(tab.title).toContain(`cli-repo-${repoCount}`);
      expect(tab.title).toContain("working tree");
      expect(state.focused_tab).toBe(tab.review_id);

      // A second tab, then a `polygloss://` URL brings the first one back.
      const other = makeRepo(world.env, world.home);
      const third = await secondInstance(world.env, [
        "--repo",
        other,
        "--live",
      ]);
      expect(third.exitCode).toBe(0);
      await waitForState(world.socket, (s) =>
        s.tabs.length === before.tabs.length + 2 &&
        s.focused_tab !== tab.review_id
          ? s
          : null,
      );
      const url = await secondInstance(world.env, [
        `polygloss://review/${tab.review_id}`,
      ]);
      expect(url.exitCode).toBe(0);
      const back = await waitForState(world.socket, (s) =>
        s.focused_tab === tab.review_id ? s : null,
      );
      expect(back.tabs.length).toBe(before.tabs.length + 2);
      // Still one app: every forwarder exited, the original answers.
      expect(await waitForApp(world.socket)).toBe(appPid);
    });

    test("polygloss open activates the app", async () => {
      const repo = makeRepo(world.env, world.home);
      // A commit review recorded without the app, then another tab in front.
      const shown = cli(world.env, ["--no-open", "show", "HEAD"], repo);
      expect(shown.exitCode).toBe(0);
      expect(shown.json.app).toBe("skipped");
      const live = cli(world.env, ["--repo", repo], repo);
      expect(live.exitCode).toBe(0);
      expect(live.json.app).toBe("opened");
      const behind = await waitForState(world.socket, (s) =>
        s.focused_tab === live.json.review_id ? s : null,
      );

      const opened = cli(world.env, ["open", shown.json.diff_id.slice(0, 10)]);
      expect(opened.stderr).toBe("");
      expect(opened.exitCode).toBe(0);
      expect(opened.json).toMatchObject({
        review_id: shown.json.review_id,
        diff_id: shown.json.diff_id,
        app: "opened",
      });
      // The diff's review comes to the front in its own tab, and the app asks
      // macOS to activate it once. Whether macOS complies is its call
      // (cooperative activation; nothing activates while the screen is
      // locked), so `app_active` is not asserted.
      const state = await waitForState(world.socket, (s) =>
        s.focused_tab === shown.json.review_id ? s : null,
      );
      expect(state.window_open).toBe(true);
      expect(state.tabs.find((t) => t.active)?.diff_id).toBe(
        shown.json.diff_id,
      );
      expect(state.tabs.length).toBe(behind.tabs.length + 1);
      expect(state.activations).toBe(behind.activations + 1);
    });
  },
);
