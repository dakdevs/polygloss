// Per-test sandbox: a throwaway HOME, data dir, config dir, cache dir and git
// config so no test ever reads or writes the real ~/Library, ~/.config or git
// config (plan: Global constraints, "Test hygiene").
import {
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** Fixed git identity and dates so commits made in a sandbox are reproducible. */
export const fixedGitIdentity: Record<string, string> = {
  GIT_AUTHOR_NAME: "Polygloss Test",
  GIT_AUTHOR_EMAIL: "test@polygloss.invalid",
  GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z",
  GIT_COMMITTER_NAME: "Polygloss Test",
  GIT_COMMITTER_EMAIL: "test@polygloss.invalid",
  GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z",
};

// Only these are inherited from the test runner; everything else (GIT_DIR,
// XDG_*, POLYGLOSS_*, …) is dropped so it cannot point back at real state.
const inherited = [
  "PATH",
  "TMPDIR",
  "USER",
  "LOGNAME",
  "SHELL",
  "TERM",
  "LANG",
];

export function makeSandbox(): {
  home: string;
  dataDir: string;
  configDir: string;
  env: Record<string, string>;
  cleanup: () => void;
} {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "polygloss-sandbox-")));
  const home = join(root, "home");
  const dataDir = join(root, "data");
  const configDir = join(home, ".config");
  const cacheDir = join(home, ".cache");
  for (const dir of [home, dataDir, configDir, cacheDir])
    mkdirSync(dir, { recursive: true, mode: 0o700 });
  const gitConfig = join(root, "gitconfig");
  writeFileSync(gitConfig, "");

  const env: Record<string, string> = {};
  for (const key of inherited) {
    const value = process.env[key];
    if (value !== undefined) env[key] = value;
  }
  Object.assign(env, fixedGitIdentity, {
    HOME: home,
    POLYGLOSS_DATA_DIR: dataDir,
    XDG_CONFIG_HOME: configDir,
    XDG_CACHE_HOME: cacheDir,
    GIT_CONFIG_GLOBAL: gitConfig,
    GIT_CONFIG_NOSYSTEM: "1",
  });

  return {
    home,
    dataDir,
    configDir,
    env,
    cleanup: () => rmSync(root, { recursive: true, force: true }),
  };
}
