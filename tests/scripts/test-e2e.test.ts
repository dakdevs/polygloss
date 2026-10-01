import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
let cargoHome = "";
let targetDir = "";
let log = "";

// A fake rustup cargo: logs argv, "builds" Polygloss, fails on FAKE_CARGO_FAIL.
const fakeCargo = `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$FAKE_CARGO_LOG"
case "$*" in *"$FAKE_CARGO_FAIL"*) echo "fake failure: $*" >&2; exit 101 ;; esac
if [ "$1" = build ] && [ -n "\${FAKE_CARGO_MAKE_APP-}" ]; then
  mkdir -p "$CARGO_TARGET_DIR/debug" && : >"$CARGO_TARGET_DIR/debug/Polygloss"
fi
if [ "$1" = packager ] && [ -n "\${FAKE_CARGO_MAKE_BUNDLE-}" ]; then
  out=""
  while [ $# -gt 0 ]; do [ "$1" = --out-dir ] && out="$2"; shift; done
  mkdir -p "$out/Polygloss.app/Contents/MacOS" "$out/Polygloss.app/Contents/Resources"
  cp /usr/bin/true "$out/Polygloss.app/Contents/MacOS/Polygloss"
  cp /usr/bin/true "$out/Polygloss.app/Contents/MacOS/polygloss-cli"
  cp "$FAKE_CARGO_MAKE_BUNDLE" "$out/Polygloss.app/Contents/Info.plist"
fi
`;

// A fake lsregister (POLYGLOSS_LSREGISTER): logs argv. No test here touches
// the user's real LaunchServices database.
let fakeLsregister = "";

beforeAll(() => {
  cargoHome = join(sandbox.home, "cargo-home");
  mkdirSync(join(cargoHome, "bin"), { recursive: true });
  writeFileSync(join(cargoHome, "bin", "cargo"), fakeCargo);
  chmodSync(join(cargoHome, "bin", "cargo"), 0o755);
  fakeLsregister = join(sandbox.home, "fake-lsregister");
  writeFileSync(
    fakeLsregister,
    `#!/usr/bin/env bash\nprintf 'lsregister %s\\n' "$*" >>"$FAKE_CARGO_LOG"\n`,
  );
  chmodSync(fakeLsregister, 0o755);
});

afterAll(() => sandbox.cleanup());

// The fake build makes an empty `Polygloss` and no CLI, so only the smoke
// suite can run against it (the other suites drive the real app).
const SMOKE = "tests/e2e/smoke.test.ts";

function runE2e(
  env: Record<string, string>,
  args: string[] = [SMOKE],
): {
  exitCode: number;
  output: string;
  log: string[];
} {
  targetDir = join(
    sandbox.home,
    `target-${Math.random().toString(36).slice(2)}`,
  );
  log = join(sandbox.home, "cargo.log");
  rmSync(log, { force: true });
  const r = Bun.spawnSync([join(repoRoot, "scripts", "test-e2e.sh"), ...args], {
    cwd: sandbox.home,
    env: {
      ...sandbox.env,
      CARGO_HOME: cargoHome,
      CARGO_TARGET_DIR: targetDir,
      CARGO_BUILD_BUILD_DIR: join(sandbox.home, "target-shared"),
      FAKE_CARGO_LOG: log,
      FAKE_CARGO_FAIL: "<no-such-step>",
      POLYGLOSS_LSREGISTER: fakeLsregister,
      ...env,
    },
  });
  return {
    exitCode: r.exitCode ?? -1,
    output: r.stdout.toString() + r.stderr.toString(),
    log: existsSync(log)
      ? readFileSync(log, "utf8").split("\n").filter(Boolean)
      : [],
  };
}

describe("scripts/test-e2e.sh", () => {
  test("builds each binary on its own, runs the e2e nextest binary, then the bun E2E suite", () => {
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1" });
    expect(r.output).toContain("app E2E smoke");
    expect(r.exitCode).toBe(0);
    expect(r.log).toEqual([
      "build -p polygloss-app",
      "build -p polygloss-cli",
      "nextest run -p polygloss-app --features e2e --no-tests=warn -E binary(e2e)",
    ]);
  });

  test("the bun E2E suite runs with POLYGLOSS_E2E set and fails without the app", () => {
    const r = runE2e({});
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("appBin() points at the built Polygloss app");
    expect(r.log.length).toBe(3);
  });

  test("without arguments the bun step runs every suite in tests/e2e", () => {
    // Against the fake (empty, non-executable) app every app suite fails
    // fast, but each one runs.
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1" }, []);
    expect(r.exitCode).not.toBe(0);
    for (const suite of [
      "smoke.test.ts",
      "mcp-app.test.ts",
      "cli-app.test.ts",
      "mcp-multi-process-writers.test.ts",
    ])
      expect(r.output).toContain(`tests/e2e/${suite}`);
  });

  test("with POLYGLOSS_BUNDLE_E2E=1 it then packages the release bundle", () => {
    // The fake packager makes no bundle, so package-release.sh stops there;
    // the bundle suite itself is tests/scripts/package.test.ts.
    const dist = join(sandbox.home, "dist-bundle-e2e");
    const r = runE2e({
      FAKE_CARGO_MAKE_APP: "1",
      POLYGLOSS_BUNDLE_E2E: "1",
      POLYGLOSS_DIST_DIR: dist,
    });
    expect(r.output).toContain("app E2E smoke");
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("cargo packager made no");
    expect(r.log.slice(3)).toEqual([
      "build --release -p polygloss-app",
      "build --release -p polygloss-cli",
      `packager --release --formats app --out-dir ${dist}`,
    ]);
  });

  test("by default the test bundle goes to <target>/bundle-e2e.noindex, which Spotlight skips", () => {
    // Spotlight registers a new app bundle with LaunchServices on its own a
    // minute or so after it is written; it never looks inside `.noindex`.
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1", POLYGLOSS_BUNDLE_E2E: "1" });
    expect(r.exitCode).not.toBe(0);
    expect(r.log.at(-1)).toBe(
      `packager --release --formats app --out-dir ${realpathSync(targetDir)}/bundle-e2e.noindex`,
    );
  });

  /**
   * Env for a bundle run: a fake cargo whose packager lays out a bundle (real
   * Mach-O stand-ins, so package-release.sh signs, stamps and checks it for
   * real) and a fake bun that records each suite run and the binaries it
   * would test, failing on the bundle run when FAKE_BUN_FAIL_BUNDLE is set.
   */
  function bundleRunEnv(dist: string): Record<string, string> {
    const bin = join(sandbox.home, "fake-bin");
    mkdirSync(bin, { recursive: true });
    writeFileSync(
      join(bin, "bun"),
      `#!/usr/bin/env bash
printf 'bun %s app=%s cli=%s bundle=%s e2e=%s\\n' "$*" "\${POLYGLOSS_APP_BIN-}" "\${POLYGLOSS_CLI_BIN-}" "\${POLYGLOSS_BUNDLE-}" "\${POLYGLOSS_E2E-}" >>"$FAKE_CARGO_LOG"
if [ -n "\${FAKE_BUN_FAIL_BUNDLE-}" ] && [ -n "\${POLYGLOSS_APP_BIN-}" ]; then
  echo "fake failure: bundle E2E" >&2; exit 1
fi
`,
    );
    chmodSync(join(bin, "bun"), 0o755);
    const plistJson = join(sandbox.home, "packager-info.json");
    const plist = join(sandbox.home, "packager-info.plist");
    const ours = JSON.parse(
      Bun.spawnSync([
        "plutil",
        "-convert",
        "json",
        "-o",
        "-",
        join(repoRoot, "packaging", "Info.plist"),
      ]).stdout.toString(),
    );
    const version = (
      Bun.TOML.parse(
        readFileSync(join(repoRoot, "Cargo.toml"), "utf8"),
      ) as Record<string, any>
    ).workspace.package.version as string;
    writeFileSync(
      plistJson,
      JSON.stringify({
        CFBundleExecutable: "Polygloss",
        CFBundlePackageType: "APPL",
        CFBundleShortVersionString: version,
        ...ours,
      }),
    );
    Bun.spawnSync(["plutil", "-convert", "xml1", "-o", plist, plistJson]);
    return {
      FAKE_CARGO_MAKE_APP: "1",
      FAKE_CARGO_MAKE_BUNDLE: plist,
      POLYGLOSS_BUNDLE_E2E: "1",
      POLYGLOSS_DIST_DIR: dist,
      PATH: `${bin}:${process.env.PATH}`,
    };
  }

  test("with POLYGLOSS_BUNDLE_E2E=1 the E2E suites run again against the bundle's executables", () => {
    const dist = join(sandbox.home, "dist-bundle-run");
    const r = runE2e(bundleRunEnv(dist));
    expect(r.output).toContain("package-release: done");
    expect(r.exitCode).toBe(0);
    const app = join(realpathSync(dist), "Polygloss.app");
    const macos = join(app, "Contents", "MacOS");
    expect(
      r.log.filter((l) => l.startsWith("bun ") || l.startsWith("lsregister ")),
    ).toEqual([
      `bun test ${SMOKE} app= cli= bundle= e2e=1`,
      `bun test tests/scripts/package.test.ts app= cli= bundle=${app} e2e=`,
      // The same suites (the path filter carries over), now on the bundle.
      `bun test ${SMOKE} app=${macos}/Polygloss cli=${macos}/polygloss-cli bundle= e2e=1`,
      // Then the bundle leaves the user's LaunchServices database.
      `lsregister -u ${app}`,
    ]);
    // A caller-provided dist dir is the caller's: the bundle stays.
    expect(existsSync(app)).toBe(true);
  }, 120_000);

  test("the default test bundle is deleted after the run, pass or fail", () => {
    // macOS 26 registers an unregistered bundle again within a second while
    // it still exists (plan T5.8), so only deleting it keeps it out of the
    // LaunchServices database. The DMG stays.
    for (const fail of [false, true]) {
      const env = bundleRunEnv(join(sandbox.home, "unused"));
      delete env.POLYGLOSS_DIST_DIR;
      const r = runE2e(fail ? { ...env, FAKE_BUN_FAIL_BUNDLE: "1" } : env);
      expect(r.exitCode).toBe(fail ? 1 : 0);
      const dist = join(realpathSync(targetDir), "bundle-e2e.noindex");
      const app = join(dist, "Polygloss.app");
      expect(r.log.at(-1)).toBe(`lsregister -u ${app}`);
      expect(existsSync(app)).toBe(false);
      expect(existsSync(dist)).toBe(true);
    }
  }, 240_000);

  test("a failing bundle run still unregisters the bundle from LaunchServices", () => {
    const dist = join(sandbox.home, "dist-bundle-fail");
    const r = runE2e({ ...bundleRunEnv(dist), FAKE_BUN_FAIL_BUNDLE: "1" });
    expect(r.output).toContain("fake failure: bundle E2E");
    // The trap keeps the failing step's exit status.
    expect(r.exitCode).toBe(1);
    const app = join(realpathSync(dist), "Polygloss.app");
    expect(r.log.at(-1)).toBe(`lsregister -u ${app}`);
    expect(r.log.filter((l) => l.startsWith("lsregister "))).toEqual([
      `lsregister -u ${app}`,
    ]);
  }, 120_000);

  test("stops at the first failing step", () => {
    const r = runE2e({ FAKE_CARGO_MAKE_APP: "1", FAKE_CARGO_FAIL: "nextest" });
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("fake failure: nextest");
    expect(r.output).not.toContain("app E2E smoke");
  });
});
