// Packaging (plan T5.1, design §21, library-choices §13): the cargo-packager
// config, scripts/package-release.sh (against a fake cargo whose `packager`
// lays out a bundle the way cargo-packager does, with real Mach-O stand-ins, so
// the version stamp, ad-hoc signature and DMG are real), the static checks of
// scripts/smoke-bundle.sh, the icon, and, with POLYGLOSS_BUNDLE_E2E=1, the real
// release bundle (scripts/test-e2e.sh builds it with package-release.sh first).
// Every process runs in a sandbox; the bundle runs from its build dir, never
// from /Applications.
import {
  afterAll,
  beforeAll,
  describe,
  expect,
  setDefaultTimeout,
  test,
} from "bun:test";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import {
  appCall,
  debugState,
  git,
  stopAppPid,
  waitForApp,
  waitForState,
} from "../support/app";
import { makeSandbox } from "../support/sandbox";
import { makeSparkleFramework } from "../support/sparkle-fixture";

setDefaultTimeout(120_000);

type Json = Record<string, any>;
type Env = Record<string, string>;

const repoRoot = resolve(import.meta.dir, "../..");
const packageRelease = join(repoRoot, "scripts", "package-release.sh");
const smokeBundle = join(repoRoot, "scripts", "smoke-bundle.sh");
const makeIcon = join(repoRoot, "scripts", "make-icon.sh");
const lsregister =
  "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

const workspaceVersion: string = (
  Bun.TOML.parse(readFileSync(join(repoRoot, "Cargo.toml"), "utf8")) as Json
).workspace.package.version;

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

let scratchCount = 0;
/** A fresh directory inside the sandbox. */
function scratch(name: string): string {
  scratchCount += 1;
  const dir = join(sandbox.home, `${name}-${scratchCount}`);
  mkdirSync(dir, { recursive: true });
  return dir;
}

function run(
  argv: string[],
  opts: { env?: Env; cwd?: string } = {},
): { exitCode: number; stdout: string; stderr: string; output: string } {
  const r = Bun.spawnSync(argv, {
    env: { ...sandbox.env, ...opts.env },
    cwd: opts.cwd ?? sandbox.home,
  });
  const stdout = r.stdout.toString();
  const stderr = r.stderr.toString();
  return {
    exitCode: r.exitCode ?? -1,
    stdout,
    stderr,
    output: stdout + stderr,
  };
}

function must(argv: string[], opts: { env?: Env; cwd?: string } = {}): string {
  const r = run(argv, opts);
  if (r.exitCode !== 0)
    throw new Error(`${argv.join(" ")} failed (${r.exitCode}): ${r.output}`);
  return r.stdout;
}

/** An Info.plist (any plist) as JSON. */
function readPlist(path: string): Json {
  return JSON.parse(must(["plutil", "-convert", "json", "-o", "-", path]));
}

function writePlist(path: string, value: Json): void {
  writeFileSync(`${path}.json`, JSON.stringify(value));
  must(["plutil", "-convert", "xml1", "-o", path, `${path}.json`]);
  rmSync(`${path}.json`);
}

/** The Info.plist keys cargo-packager generates, then ours merged over them. */
function packagerInfoPlist(overrides: Json = {}): Json {
  return {
    CFBundleDevelopmentRegion: "English",
    CFBundleDisplayName: "Polygloss",
    CFBundleExecutable: "Polygloss",
    CFBundleIconFile: "icon.icns",
    CFBundleIdentifier: "dev.dak.polygloss",
    CFBundleInfoDictionaryVersion: "6.0",
    CFBundleName: "Polygloss",
    CFBundlePackageType: "APPL",
    CFBundleShortVersionString: workspaceVersion,
    // cargo-packager's default build number: a UTC timestamp.
    CFBundleVersion: "20260930.120000",
    ...readPlist(join(repoRoot, "packaging", "Info.plist")),
    ...overrides,
  };
}

/** An appcast URL and an Ed25519 public key (32 bytes, base64) for tests. */
const APPCAST_URL = "https://example.invalid/polygloss/appcast.xml";
const PUBLIC_ED_KEY = Buffer.alloc(32, 7).toString("base64");
const sparkleKeys = { SUFeedURL: APPCAST_URL, SUPublicEDKey: PUBLIC_ED_KEY };
const adhocEntitlements = join(
  repoRoot,
  "packaging",
  "entitlements-adhoc.plist",
);

/** What package-release.sh copies into Contents/Resources (plan T5.7). */
const bundledLicenses: [string, string][] = [
  ["LICENSE-MIT", join(repoRoot, "LICENSE-MIT")],
  ["LICENSE-APACHE", join(repoRoot, "LICENSE-APACHE")],
  ["NOTICE", join(repoRoot, "NOTICE")],
  [
    "third-party-notices.md",
    join(repoRoot, "packaging", "third-party-notices.md"),
  ],
];

/**
 * A bundle as package-release.sh leaves it: copies of /usr/bin/true (a real
 * Mach-O) as both executables, an Info.plist with the stamped version
 * (unless `plist`), with a stand-in Sparkle.framework (`sparkle`, signed
 * ad-hoc inside out), ad-hoc signed (unless `sign: false`), with
 * `entitlements` (a plist path) on the bundle. `omit` leaves out
 * executables or bundled license files by name.
 */
function makeBundle(
  dir: string,
  opts: {
    omit?: string[];
    plist?: Json;
    sign?: boolean;
    sparkle?: boolean;
    entitlements?: string;
  } = {},
): string {
  const app = join(dir, "Polygloss.app");
  mkdirSync(join(app, "Contents", "MacOS"), { recursive: true });
  mkdirSync(join(app, "Contents", "Resources"), { recursive: true });
  for (const exe of ["Polygloss", "polygloss-cli"]) {
    if (opts.omit?.includes(exe)) continue;
    copyFileSync("/usr/bin/true", join(app, "Contents", "MacOS", exe));
    chmodSync(join(app, "Contents", "MacOS", exe), 0o755);
  }
  writePlist(
    join(app, "Contents", "Info.plist"),
    opts.plist ?? packagerInfoPlist({ CFBundleVersion: workspaceVersion }),
  );
  for (const [name, src] of bundledLicenses)
    if (!opts.omit?.includes(name))
      copyFileSync(src, join(app, "Contents", "Resources", name));
  if (opts.sparkle) {
    const frameworks = join(app, "Contents", "Frameworks");
    mkdirSync(frameworks, { recursive: true });
    const fw = makeSparkleFramework(frameworks);
    if (opts.sign !== false)
      must([join(repoRoot, "scripts", "sign-sparkle.sh"), fw, "-"]);
  }
  if (opts.sign !== false) {
    for (const exe of ["polygloss-cli"])
      if (!opts.omit?.includes(exe))
        must([
          "codesign",
          "--force",
          "--sign",
          "-",
          join(app, "Contents", "MacOS", exe),
        ]);
    must([
      "codesign",
      "--force",
      "--sign",
      "-",
      ...(opts.entitlements ? ["--entitlements", opts.entitlements] : []),
      app,
    ]);
  }
  return app;
}

function smokeStatic(
  app: string,
  env: Env = {},
): {
  exitCode: number;
  output: string;
} {
  return run([smokeBundle, "--static", app], { env });
}

describe("packaging config", () => {
  const manifest = Bun.TOML.parse(
    readFileSync(
      join(repoRoot, "crates", "polygloss-app", "Cargo.toml"),
      "utf8",
    ),
  ) as Json;
  const packager = manifest.package.metadata.packager as Json;

  test("packager metadata bundles Polygloss and polygloss-cli as dev.dak.polygloss", () => {
    expect(packager["product-name"]).toBe("Polygloss");
    expect(packager.identifier).toBe("dev.dak.polygloss");
    expect(packager.binaries).toEqual([
      { path: "Polygloss", main: true },
      { path: "polygloss-cli" },
    ]);
    expect(packager["deep-link-protocols"]).toEqual([
      { schemes: ["polygloss"], role: "viewer" },
    ]);
    expect(packager.formats).toEqual(["app"]);
    expect(packager["out-dir"]).toBe("../../dist");
    expect(packager.icons).toEqual(["../../packaging/icon.icns"]);
    expect(packager.macos["minimum-system-version"]).toBe("14.0");
    expect(packager.macos["info-plist-path"]).toBe(
      "../../packaging/Info.plist",
    );
    expect(packager.macos.entitlements).toBe(
      "../../packaging/entitlements.plist",
    );
  });

  test("packager metadata never builds jointly or names a signing identity", () => {
    // A joint build unifies features (plan T5.1); signing is env-only (T5.2).
    expect(packager["before-packaging-command"]).toBeUndefined();
    expect(packager["before-each-package-command"]).toBeUndefined();
    expect(packager.macos["signing-identity"]).toBeUndefined();
    expect(JSON.stringify(manifest)).not.toContain("FCSF68W94H");
  });

  test("Info.plist sets the identifier, url scheme and minimum macOS, not the version", () => {
    const plist = readPlist(join(repoRoot, "packaging", "Info.plist"));
    expect(plist.CFBundleIdentifier).toBe("dev.dak.polygloss");
    expect(plist.LSMinimumSystemVersion).toBe("14.0");
    expect(plist.CFBundleURLTypes).toEqual([
      {
        CFBundleURLName: "dev.dak.polygloss",
        CFBundleURLSchemes: ["polygloss"],
        CFBundleTypeRole: "Viewer",
      },
    ]);
    // package-release.sh stamps both from the crate version.
    expect(plist.CFBundleVersion).toBeUndefined();
    expect(plist.CFBundleShortVersionString).toBeUndefined();
    // Sparkle's keys come from the environment at package time (T5.3), and
    // automatic checks stay unset so Sparkle asks first (OQ-16).
    expect(plist.SUFeedURL).toBeUndefined();
    expect(plist.SUPublicEDKey).toBeUndefined();
    expect(plist.SUEnableAutomaticChecks).toBeUndefined();
  });

  test("entitlements are empty: hardened runtime with no exceptions", () => {
    expect(
      readPlist(join(repoRoot, "packaging", "entitlements.plist")),
    ).toEqual({});
  });

  test("the ad-hoc entitlements only relax library validation", () => {
    expect(readPlist(adhocEntitlements)).toEqual({
      "com.apple.security.cs.disable-library-validation": true,
    });
    // The Developer ID signer never uses them.
    const signer = readFileSync(
      join(repoRoot, "scripts", "sign-and-notarize.sh"),
      "utf8",
    );
    expect(signer).not.toContain("entitlements-adhoc");
    expect(signer).toContain('/entitlements.plist"');
  });

  test("dist/ is gitignored", () => {
    expect(readFileSync(join(repoRoot, ".gitignore"), "utf8")).toContain(
      "/dist/",
    );
  });
});

describe("icon", () => {
  const sizes = [
    "icon_16x16.png",
    "icon_16x16@2x.png",
    "icon_32x32.png",
    "icon_32x32@2x.png",
    "icon_128x128.png",
    "icon_128x128@2x.png",
    "icon_256x256.png",
    "icon_256x256@2x.png",
    "icon_512x512.png",
    "icon_512x512@2x.png",
  ];

  function iconsetOf(icns: string): string[] {
    const out = join(scratch("iconset"), "out.iconset");
    must(["iconutil", "--convert", "iconset", "--output", out, icns]);
    return readdirSync(out).sort();
  }

  test("packaging/icon.icns has every size from 16 to 1024 px", () => {
    expect(iconsetOf(join(repoRoot, "packaging", "icon.icns"))).toEqual(
      [...sizes].sort(),
    );
  });

  const rasterizer = ["rsvg-convert", "magick"].some((t) => Bun.which(t));
  test.skipIf(!rasterizer)(
    "make-icon.sh renders the svg into a complete icns",
    () => {
      const out = join(scratch("make-icon"), "icon.icns");
      must([makeIcon, join(repoRoot, "assets", "icons", "polygloss.svg"), out]);
      expect(iconsetOf(out)).toEqual([...sizes].sort());
    },
  );

  test("packaging/polygloss.icon draws the svg's shapes as its layers", () => {
    // The .icns comes from the svg and Assets.car from the Icon Composer
    // document: both must show the same icon. Each layer reuses the svg's path
    // in the svg's frame; its viewBox maps the 824 body onto Icon Composer's
    // canvas, which is the squircle itself.
    const doc = join(repoRoot, "packaging", "polygloss.icon");
    const json = JSON.parse(
      readFileSync(join(doc, "icon.json"), "utf8"),
    ) as Json;
    expect(json["supported-platforms"]).toEqual({ squares: ["macOS"] });
    const images: string[] = json.groups.flatMap((g: Json) =>
      g.layers.map((l: Json) => l["image-name"] as string),
    );
    expect([...images].sort()).toEqual(readdirSync(join(doc, "Assets")).sort());
    const svg = readFileSync(
      join(repoRoot, "assets", "icons", "polygloss.svg"),
      "utf8",
    );
    const shapes = new Map(
      [...svg.matchAll(/<path id="([a-z-]+)" d="([^"]+)"/g)].map((m) => [
        m[1],
        m[2],
      ]),
    );
    expect([...shapes.keys()].sort()).toEqual(["added", "deleted", "gloss"]);
    for (const image of images) {
      const layer = readFileSync(join(doc, "Assets", image), "utf8");
      const shape = image.replace(/^\d+-/, "").replace(/\.svg$/, "");
      expect({ image, viewBox: layer.match(/viewBox="([^"]+)"/)?.[1] }).toEqual(
        { image, viewBox: "100 100 824 824" },
      );
      expect({ image, d: layer.match(/ d="([^"]+)"/)?.[1] }).toEqual({
        image,
        d: shapes.get(shape),
      });
    }
  });

  test("make-icon.sh rejects a missing svg", () => {
    const r = run([
      makeIcon,
      join(sandbox.home, "nope.svg"),
      join(sandbox.home, "x.icns"),
    ]);
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("nope.svg");
  });
});

describe("scripts/smoke-bundle.sh --static", () => {
  test("bundle contains Polygloss and polygloss-cli", () => {
    const ok = smokeStatic(makeBundle(scratch("ok")));
    expect(ok.output).toContain("smoke-bundle: static checks passed");
    expect(ok.exitCode).toBe(0);

    for (const exe of ["polygloss-cli", "Polygloss"]) {
      const r = smokeStatic(makeBundle(scratch("omit"), { omit: [exe] }));
      expect(r.exitCode).not.toBe(0);
      expect(r.output).toContain(`Contents/MacOS/${exe}`);
    }
  });

  test("info plist has identifier url scheme and explicit version", () => {
    const cases: [string, Json][] = [
      ["CFBundleIdentifier", { CFBundleIdentifier: "com.example.polygloss" }],
      ["CFBundleURLTypes", { CFBundleURLTypes: [] }],
      // cargo-packager's timestamp build number: Sparkle compares it.
      ["CFBundleVersion", { CFBundleVersion: "20260930.120000" }],
      ["CFBundleVersion", { CFBundleVersion: "0.0.9" }],
      ["LSMinimumSystemVersion", { LSMinimumSystemVersion: "13.0" }],
      ["CFBundleExecutable", { CFBundleExecutable: "polygloss-cli" }],
    ];
    for (const [key, overrides] of cases) {
      const plist = packagerInfoPlist({
        CFBundleVersion: workspaceVersion,
        ...overrides,
      });
      const r = smokeStatic(makeBundle(scratch("plist"), { plist }));
      expect({ key, exitCode: r.exitCode }).toEqual({
        key,
        exitCode: 1,
      });
      expect(r.output).toContain(key);
    }
  });

  test("finds the polygloss scheme in any URL type, and only there", () => {
    const types = (schemes: string[][]) =>
      packagerInfoPlist({
        CFBundleVersion: workspaceVersion,
        CFBundleURLTypes: schemes.map((CFBundleURLSchemes, i) => ({
          CFBundleURLName: `type-${i}`,
          CFBundleURLSchemes,
        })),
      });
    const second = smokeStatic(
      makeBundle(scratch("schemes"), {
        plist: types([["x-other"], ["a", "polygloss"]]),
      }),
    );
    expect(second.output).toContain("polygloss:// scheme");
    expect(second.exitCode).toBe(0);
    const none = smokeStatic(
      makeBundle(scratch("schemes"), { plist: types([["polygloss-dev"], []]) }),
    );
    expect(none.exitCode).toBe(1);
    expect(none.output).toContain("does not register the polygloss scheme");
  });

  test("a plutil that prints its errors on stdout, as macOS 15's does, changes nothing", () => {
    // The CI runner's plutil: "Could not extract value" on stdout, exit 1.
    const bin = scratch("macos15-plutil");
    writeFileSync(
      join(bin, "plutil"),
      '#!/bin/bash\nexec /usr/bin/plutil "$@" 2>&1\n',
    );
    chmodSync(join(bin, "plutil"), 0o755);
    const env = { PATH: `${bin}:${process.env.PATH}` };
    const ok = smokeStatic(makeBundle(scratch("macos15")), env);
    expect(ok.output).toContain("no updater (built without an appcast)");
    expect(ok.output).toContain("smoke-bundle: static checks passed");
    expect(ok.exitCode).toBe(0);
    const noScheme = smokeStatic(
      makeBundle(scratch("macos15"), {
        plist: packagerInfoPlist({
          CFBundleVersion: workspaceVersion,
          CFBundleURLTypes: [],
        }),
      }),
      env,
    );
    expect(noScheme.exitCode).toBe(1);
    expect(noScheme.output).toContain("does not register the polygloss scheme");
  });

  test("a release version passes when both version keys carry it", () => {
    const calver = { CFBundleShortVersionString: "20261005.3" };
    const ok = smokeStatic(
      makeBundle(scratch("calver"), {
        plist: packagerInfoPlist({ ...calver, CFBundleVersion: "20261005.3" }),
      }),
    );
    expect(ok.output).toContain("dev.dak.polygloss 20261005.3");
    expect(ok.exitCode).toBe(0);
    for (const CFBundleVersion of ["20261005.4", workspaceVersion]) {
      const r = smokeStatic(
        makeBundle(scratch("calver"), {
          plist: packagerInfoPlist({ ...calver, CFBundleVersion }),
        }),
      );
      expect({ CFBundleVersion, exitCode: r.exitCode }).toEqual({
        CFBundleVersion,
        exitCode: 1,
      });
    }
  });

  test("a bundle without its licenses or third-party notices fails", () => {
    for (const [name] of bundledLicenses) {
      const r = smokeStatic(
        makeBundle(scratch("no-license"), { omit: [name] }),
      );
      expect({ name, exitCode: r.exitCode }).toEqual({ name, exitCode: 1 });
      expect(r.output).toContain(`Contents/Resources/${name}`);
    }
  });

  test("an unsealed bundle fails the signature check", () => {
    const r = smokeStatic(makeBundle(scratch("unsigned"), { sign: false }));
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("signature");
  });

  test("sparkle is embedded exactly when the feed keys are set", () => {
    const withBoth = packagerInfoPlist({
      CFBundleVersion: workspaceVersion,
      ...sparkleKeys,
    });
    const ok = smokeStatic(
      makeBundle(scratch("sparkle"), {
        plist: withBoth,
        sparkle: true,
        entitlements: adhocEntitlements,
      }),
    );
    expect(ok.output).toContain("smoke-bundle: ok: Sparkle embedded");
    expect(ok.output).toContain("library validation relaxed");
    expect(ok.exitCode).toBe(0);
    expect(smokeStatic(makeBundle(scratch("plain"))).output).toContain(
      "no updater",
    );

    const noKeys = smokeStatic(
      makeBundle(scratch("sparkle-no-keys"), { sparkle: true }),
    );
    expect(noKeys.exitCode).toBe(1);
    expect(noKeys.output).toContain("lacks SUFeedURL or SUPublicEDKey");
    for (const key of ["SUFeedURL", "SUPublicEDKey"]) {
      const plist = packagerInfoPlist({
        CFBundleVersion: workspaceVersion,
        [key]: (sparkleKeys as Json)[key],
      });
      const r = smokeStatic(makeBundle(scratch("keys-only"), { plist }));
      expect({ key, exitCode: r.exitCode }).toEqual({ key, exitCode: 1 });
      expect(r.output).toContain("no Sparkle.framework is embedded");
    }
  });

  test("library validation is only relaxed for an ad-hoc bundle with sparkle", () => {
    const r = smokeStatic(
      makeBundle(scratch("dlv"), { entitlements: adhocEntitlements }),
    );
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain(
      "library validation is disabled without Sparkle embedded",
    );
  });

  test("usage errors exit 2", () => {
    expect(run([smokeBundle]).exitCode).toBe(2);
    expect(run([smokeBundle, "--bogus", "x.app"]).exitCode).toBe(2);
    expect(run([smokeBundle, join(sandbox.home, "none.app")]).exitCode).toBe(1);
  });
});

// A fake rustup cargo: logs argv; `packager` lays out the unsigned bundle
// cargo-packager would (its Info.plist from FAKE_PACKAGER_PLIST) in --out-dir.
const fakeCargo = `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$FAKE_CARGO_LOG"
case "$*" in *"$FAKE_CARGO_FAIL"*) echo "fake failure: $*" >&2; exit 101 ;; esac
if [ "$1" = build ]; then
  printf 'version-at-build:%s\\n' "\${POLYGLOSS_VERSION-}" >>"$FAKE_CARGO_LOG.env"
fi
if [ "$1" = build ] && [ -n "\${FAKE_CARGO_PAUSE-}" ] && [ ! -e "$FAKE_CARGO_PAUSE" ]; then
  : >"$FAKE_CARGO_PAUSE"
  sleep 1
fi
if [ "$1" = packager ]; then
  if [ -n "\${FAKE_CARGO_ENV_LOG-}" ]; then
    printf 'env-at-packager:%s\\n' "$(env | grep -o '^APPLE_[A-Z_]*' | sort | tr '\\n' ' ' | sed 's/ $//')" >>"$FAKE_CARGO_LOG"
  fi
  out=""
  while [ $# -gt 0 ]; do
    if [ "$1" = --out-dir ]; then out="$2"; fi
    shift
  done
  app="$out/Polygloss.app"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp /usr/bin/true "$app/Contents/MacOS/Polygloss"
  cp /usr/bin/true "$app/Contents/MacOS/polygloss-cli"
  cp "$FAKE_PACKAGER_PLIST" "$app/Contents/Info.plist"
fi
`;

// `xcrun actool <doc> ... --compile <dir> ...` stand-in (see noXcodePath).
const fakeActoolXcrun = `#!/usr/bin/env bash
set -euo pipefail
[ "$1" = actool ] || { echo "fake xcrun: unexpected $1" >&2; exit 1; }
shift
printf '%s\\n' "$@" >"$FAKE_ACTOOL_LOG"
printf '%s\\n' "\${DEVELOPER_DIR-<unset>}" >"$FAKE_ACTOOL_LOG.developer-dir"
while [ $# -gt 0 ]; do
  if [ "$1" = --compile ]; then echo "stand-in catalog" >"$2/Assets.car"; fi
  shift
done
`;

describe("scripts/package-release.sh", () => {
  let cargoHome = "";
  let packagerPlist = "";
  // PATHs whose `xcrun` stands in for Xcode (the real actool's helper daemon
  // reads and logs into the real ~/Library): one finds no developer tools, as
  // on a machine without Xcode, the other is an actool that logs its
  // arguments to $FAKE_ACTOOL_LOG and writes a stand-in Assets.car.
  let noXcodePath = "";
  let fakeActoolPath = "";

  beforeAll(() => {
    cargoHome = scratch("cargo-home");
    mkdirSync(join(cargoHome, "bin"), { recursive: true });
    writeFileSync(join(cargoHome, "bin", "cargo"), fakeCargo);
    chmodSync(join(cargoHome, "bin", "cargo"), 0o755);
    packagerPlist = join(cargoHome, "packager-info.plist");
    writePlist(packagerPlist, packagerInfoPlist());
    const noXcode = join(cargoHome, "no-xcode");
    mkdirSync(noXcode);
    writeFileSync(
      join(noXcode, "xcrun"),
      "#!/bin/sh\necho 'xcrun: error: no developer tools were found' >&2\nexit 1\n",
    );
    chmodSync(join(noXcode, "xcrun"), 0o755);
    noXcodePath = `${noXcode}:${sandbox.env.PATH}`;
    const fakeActool = join(cargoHome, "fake-actool");
    mkdirSync(fakeActool);
    writeFileSync(join(fakeActool, "xcrun"), fakeActoolXcrun);
    chmodSync(join(fakeActool, "xcrun"), 0o755);
    fakeActoolPath = `${fakeActool}:${sandbox.env.PATH}`;
  });

  function packageRun(
    args: string[] = [],
    env: Env = {},
  ): {
    exitCode: number;
    output: string;
    log: string[];
    envLog: string[];
    dist: string;
  } {
    const dist = join(scratch("dist"), "dist");
    const log = join(scratch("log"), "cargo.log");
    const r = run([packageRelease, ...args], {
      env: {
        CARGO_HOME: cargoHome,
        CARGO_TARGET_DIR: join(sandbox.home, "target"),
        CARGO_BUILD_BUILD_DIR: join(sandbox.home, "target-shared"),
        POLYGLOSS_DIST_DIR: dist,
        FAKE_CARGO_LOG: log,
        FAKE_CARGO_FAIL: "<no-such-step>",
        FAKE_PACKAGER_PLIST: packagerPlist,
        PATH: noXcodePath,
        ...env,
      },
    });
    const lines = (path: string) =>
      existsSync(path)
        ? readFileSync(path, "utf8").split("\n").filter(Boolean)
        : [];
    return {
      exitCode: r.exitCode,
      output: r.output,
      log: lines(log),
      envLog: lines(`${log}.env`),
      dist,
    };
  }

  test("builds each binary with its own -p in release, then packages the app", () => {
    const r = packageRun();
    expect(r.output).toContain("package-release: done");
    expect(r.exitCode).toBe(0);
    expect(r.log).toEqual([
      "build --release -p polygloss-app",
      "build --release -p polygloss-cli",
      `packager --release --formats app --out-dir ${r.dist}`,
    ]);
  });

  test("stamps CFBundleVersion from the crate version and ad-hoc signs with the hardened runtime", () => {
    const r = packageRun();
    expect(r.exitCode).toBe(0);
    const app = join(r.dist, "Polygloss.app");
    const plist = readPlist(join(app, "Contents", "Info.plist"));
    expect(plist.CFBundleShortVersionString).toBe(workspaceVersion);
    expect(plist.CFBundleVersion).toBe(workspaceVersion);
    must(["codesign", "--verify", "--deep", "--strict", app]);
    for (const [path, identifier] of [
      [app, "dev.dak.polygloss"],
      [
        join(app, "Contents", "MacOS", "polygloss-cli"),
        "dev.dak.polygloss.cli",
      ],
    ] as const) {
      const info = run(["codesign", "--display", "--verbose=2", path]).output;
      expect(info).toContain(`Identifier=${identifier}`);
      expect(info).toContain("Signature=adhoc");
      expect(info).toMatch(/flags=0x[0-9a-f]+\([^)]*runtime[^)]*\)/);
    }
  });

  test("a release version (POLYGLOSS_VERSION) reaches both builds, both plist keys and the DMG name", () => {
    const r = packageRun([], { POLYGLOSS_VERSION: "20261005.12" });
    expect(r.output).toContain("package-release: done");
    expect(r.exitCode).toBe(0);
    const plist = readPlist(
      join(r.dist, "Polygloss.app", "Contents", "Info.plist"),
    );
    expect(plist.CFBundleShortVersionString).toBe("20261005.12");
    expect(plist.CFBundleVersion).toBe("20261005.12");
    expect(readdirSync(r.dist).filter((f) => f.endsWith(".dmg"))).toEqual([
      "Polygloss_20261005.12_aarch64.dmg",
    ]);
    // Both executables are compiled with it (polygloss_core::VERSION).
    expect(r.envLog).toEqual([
      "version-at-build:20261005.12",
      "version-at-build:20261005.12",
    ]);
  });

  test("a POLYGLOSS_VERSION that is not YYYYMMDD.N is refused before anything is built", () => {
    for (const version of [
      "0.1.0",
      "20261005",
      "20261005.0",
      "20261005.01",
      "v20261005.1",
      "2026105.1",
      "20261005.1-rc.1",
    ]) {
      const r = packageRun([], { POLYGLOSS_VERSION: version });
      expect({ version, exitCode: r.exitCode }).toEqual({
        version,
        exitCode: 1,
      });
      expect(r.output).toContain(
        `POLYGLOSS_VERSION '${version}' is not a release version (YYYYMMDD.N)`,
      );
      expect(r.log).toEqual([]);
    }
  });

  test("bundles the licenses and third-party notices in Contents/Resources", () => {
    const r = packageRun();
    expect(r.exitCode).toBe(0);
    const resources = join(r.dist, "Polygloss.app", "Contents", "Resources");
    for (const [name, src] of bundledLicenses)
      expect({
        name,
        same: readFileSync(join(resources, name)).equals(readFileSync(src)),
      }).toEqual({ name, same: true });
    // Copied before signing: the seal covers them.
    must([
      "codesign",
      "--verify",
      "--deep",
      "--strict",
      join(r.dist, "Polygloss.app"),
    ]);
  });

  test("without Xcode's actool the bundle keeps icon.icns as its only icon", () => {
    const r = packageRun();
    expect(r.output).toContain("package-release: warning: no Assets.car");
    expect(r.exitCode).toBe(0);
    const app = join(r.dist, "Polygloss.app");
    expect(existsSync(join(app, "Contents", "Resources", "Assets.car"))).toBe(
      false,
    );
    const plist = readPlist(join(app, "Contents", "Info.plist"));
    expect(plist.CFBundleIconName).toBeUndefined();
    expect(plist.CFBundleIconFile).toBe("icon.icns");
    expect(existsSync(join(r.dist, ".icon-build"))).toBe(false);
  });

  test("bundles actool's Assets.car for packaging/polygloss.icon and names it before signing", () => {
    const argsLog = join(scratch("actool"), "args");
    const r = packageRun([], {
      PATH: fakeActoolPath,
      FAKE_ACTOOL_LOG: argsLog,
    });
    expect(r.output).toContain("package-release: bundling the app icon");
    expect(r.exitCode).toBe(0);
    const app = join(r.dist, "Polygloss.app");
    const plist = readPlist(join(app, "Contents", "Info.plist"));
    const args = readFileSync(argsLog, "utf8").split("\n");
    const flag = (name: string) => args[args.indexOf(name) + 1];
    expect(args[0]).toBe(join(repoRoot, "packaging", "polygloss.icon"));
    expect(flag("--platform")).toBe("macosx");
    // The catalog's icon is the one CFBundleIconName names, built for the
    // bundle's minimum macOS; icon.icns stays the fallback.
    expect(plist.CFBundleIconName).toBe(flag("--app-icon"));
    expect(flag("--minimum-deployment-target")).toBe(
      plist.LSMinimumSystemVersion,
    );
    expect(plist.CFBundleIconFile).toBe("icon.icns");
    expect(
      readFileSync(join(app, "Contents", "Resources", "Assets.car"), "utf8"),
    ).toBe("stand-in catalog\n");
    expect(existsSync(join(r.dist, ".icon-build"))).toBe(false);
    // Added before signing: the seal covers it.
    must(["codesign", "--verify", "--deep", "--strict", app]);
    // The active Xcode's actool: no DEVELOPER_DIR of its own.
    expect(readFileSync(`${argsLog}.developer-dir`, "utf8")).toBe("<unset>\n");
  });

  test("POLYGLOSS_ACTOOL_DEVELOPER_DIR runs that Xcode's actool for the icon", () => {
    const argsLog = join(scratch("actool"), "args");
    const xcode = "/Applications/Xcode_26.3.app/Contents/Developer";
    const r = packageRun([], {
      PATH: fakeActoolPath,
      FAKE_ACTOOL_LOG: argsLog,
      POLYGLOSS_ACTOOL_DEVELOPER_DIR: xcode,
    });
    expect(r.output).toContain("package-release: bundling the app icon");
    expect(r.exitCode).toBe(0);
    expect(readFileSync(`${argsLog}.developer-dir`, "utf8")).toBe(`${xcode}\n`);
  });

  test("with POLYGLOSS_REQUIRE_APP_ICON=1 a bundle without Assets.car fails before signing or a DMG", () => {
    const r = packageRun([], { POLYGLOSS_REQUIRE_APP_ICON: "1" });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain(
      "package-release: no Assets.car: packaging/polygloss.icon needs actool from Xcode 26 or later",
    );
    // actool's own output says why.
    expect(r.output).toContain(
      "actool: xcrun: error: no developer tools were found",
    );
    expect(r.output).not.toContain("warning: no Assets.car");
    expect(r.output).not.toContain("signing ad-hoc");
    expect(readdirSync(r.dist).filter((f) => f.endsWith(".dmg"))).toEqual([]);
    expect(existsSync(join(r.dist, ".icon-build"))).toBe(false);
    // With a compiled icon the same requirement passes.
    const ok = packageRun([], {
      POLYGLOSS_REQUIRE_APP_ICON: "1",
      PATH: fakeActoolPath,
      FAKE_ACTOOL_LOG: join(scratch("actool"), "args"),
    });
    expect(ok.output).toContain("package-release: done");
    expect(ok.exitCode).toBe(0);
  });

  test("makes Polygloss_<version>_aarch64.dmg holding the signed app and an Applications link", () => {
    const r = packageRun();
    expect(r.exitCode).toBe(0);
    const dmg = join(r.dist, `Polygloss_${workspaceVersion}_aarch64.dmg`);
    expect(existsSync(dmg)).toBe(true);
    expect(existsSync(join(r.dist, ".dmg-staging"))).toBe(false);
    const mount = scratch("mount");
    must([
      "hdiutil",
      "attach",
      "-nobrowse",
      "-readonly",
      "-noautoopen",
      "-mountpoint",
      mount,
      dmg,
    ]);
    try {
      expect(readdirSync(mount).filter((n) => !n.startsWith("."))).toEqual([
        "Applications",
        "Polygloss.app",
      ]);
      expect(lstatSync(join(mount, "Applications")).isSymbolicLink()).toBe(
        true,
      );
      expect(readlinkSync(join(mount, "Applications"))).toBe("/Applications");
      must([
        "codesign",
        "--verify",
        "--deep",
        "--strict",
        join(mount, "Polygloss.app"),
      ]);
    } finally {
      must(["hdiutil", "detach", "-quiet", mount]);
    }
  });

  test("holds cargo.sh's build-dir lock from the first build to the end", async () => {
    const dist = join(scratch("dist"), "dist");
    const log = join(scratch("log"), "cargo.log");
    const pause = join(scratch("pause"), "paused");
    const env: Env = {
      ...sandbox.env,
      CARGO_HOME: cargoHome,
      CARGO_TARGET_DIR: join(sandbox.home, "target"),
      CARGO_BUILD_BUILD_DIR: join(sandbox.home, "target-shared-lock"),
      POLYGLOSS_DIST_DIR: dist,
      FAKE_CARGO_LOG: log,
      FAKE_CARGO_FAIL: "<no-such-step>",
      FAKE_PACKAGER_PLIST: packagerPlist,
      FAKE_CARGO_PAUSE: pause,
      PATH: noXcodePath,
    };
    const release = Bun.spawn([packageRelease], {
      env,
      cwd: sandbox.home,
      stdout: "ignore",
      stderr: "ignore",
    });
    while (!existsSync(pause)) await Bun.sleep(20);
    // Between the two builds another cargo.sh call (here from the same
    // checkout, outside the run) must wait for the whole run.
    const other = Bun.spawn([join(repoRoot, "scripts", "cargo.sh"), "check"], {
      env,
      cwd: sandbox.home,
      stdout: "ignore",
      stderr: "ignore",
    });
    expect(await release.exited).toBe(0);
    expect(await other.exited).toBe(0);
    expect(readFileSync(log, "utf8").split("\n").filter(Boolean)).toEqual([
      "build --release -p polygloss-app",
      "build --release -p polygloss-cli",
      `packager --release --formats app --out-dir ${dist}`,
      "check",
    ]);
  });

  test("stops at the first failing step", () => {
    const r = packageRun([], { FAKE_CARGO_FAIL: "-p polygloss-cli" });
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain(
      "fake failure: build --release -p polygloss-cli",
    );
    expect(r.log.some((l) => l.startsWith("packager"))).toBe(false);
  });

  test("a failed packager step names the cargo-packager install", () => {
    const r = packageRun([], { FAKE_CARGO_FAIL: "packager" });
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain(
      "cargo install cargo-packager --version =0.11.8 --locked",
    );
  });

  test("--sign hands the app, then the dmg, to scripts/sign-and-notarize.sh", () => {
    // No credentials in the sandbox: the signer skips both (T5.2,
    // tests/scripts/sign.test.ts covers the credentialed path).
    const r = packageRun(["--sign"]);
    expect(r.output).toContain("package-release: done");
    expect(r.exitCode).toBe(0);
    const app = join(r.dist, "Polygloss.app");
    const dmg = join(r.dist, `Polygloss_${workspaceVersion}_aarch64.dmg`);
    const order = [
      `package-release: signing and notarizing ${app}`,
      `sign-and-notarize: signing skipped: no credentials (${app})`,
      `package-release: making ${dmg}`,
      `package-release: signing and notarizing ${dmg}`,
      `sign-and-notarize: signing skipped: no credentials (${dmg})`,
      "package-release: done",
    ].map((line) => r.output.indexOf(line));
    expect(order.every((i) => i >= 0)).toBe(true);
    expect([...order].sort((a, b) => a - b)).toEqual(order);
    // The ad-hoc signature stays.
    expect(run(["codesign", "--display", "--verbose=2", app]).output).toContain(
      "Signature=adhoc",
    );
  });

  test("cargo packager never sees the signing credentials", () => {
    // cargo-packager signs and imports certificates on its own when it finds
    // APPLE_CERTIFICATE (rewriting the keychain search list); T5.2 signs.
    const r = packageRun([], {
      APPLE_CERTIFICATE: "c2VjcmV0",
      APPLE_CERTIFICATE_PASSWORD: "secret",
      APPLE_SIGNING_IDENTITY: "Developer ID Application: X (5U7E4UQ5M3)",
      APPLE_API_KEY: "KEY",
      APPLE_API_ISSUER: "ISSUER",
      APPLE_API_KEY_PATH: "/nonexistent/key.p8",
      APPLE_KEYCHAIN_PROFILE: "profile",
      APPLE_ID: "someone@example.invalid",
      APPLE_PASSWORD: "secret",
      APPLE_TEAM_ID: "5U7E4UQ5M3",
      FAKE_CARGO_ENV_LOG: "1",
    });
    expect(r.exitCode).toBe(0);
    const packager = r.log.find((l) => l.startsWith("env-at-packager:"));
    expect(packager).toBe("env-at-packager:");
  });

  test("without an appcast the bundle has no updater", () => {
    const r = packageRun();
    expect(r.exitCode).toBe(0);
    expect(r.output).toContain("package-release: no updater");
    const app = join(r.dist, "Polygloss.app");
    expect(existsSync(join(app, "Contents", "Frameworks"))).toBe(false);
    const plist = readPlist(join(app, "Contents", "Info.plist"));
    expect(plist.SUFeedURL).toBeUndefined();
    expect(plist.SUPublicEDKey).toBeUndefined();
    const ents = run([
      "codesign",
      "--display",
      "--entitlements",
      "-",
      "--xml",
      app,
    ]).output;
    expect(ents).not.toContain("disable-library-validation");
  });

  test("with an appcast it embeds Sparkle, writes the feed keys and signs it inside out", () => {
    const vendor = scratch("vendor");
    const fw = makeSparkleFramework(vendor);
    const r = packageRun([], {
      POLYGLOSS_APPCAST_URL: APPCAST_URL,
      SPARKLE_PUBLIC_ED_KEY: PUBLIC_ED_KEY,
      POLYGLOSS_SPARKLE_FRAMEWORK: fw,
    });
    expect(r.output).toContain(
      `package-release: embedding Sparkle (updates from ${APPCAST_URL})`,
    );
    expect(r.output).toContain("smoke-bundle: ok: Sparkle embedded");
    expect(r.exitCode).toBe(0);
    const app = join(r.dist, "Polygloss.app");
    const plist = readPlist(join(app, "Contents", "Info.plist"));
    expect(plist.SUFeedURL).toBe(APPCAST_URL);
    expect(plist.SUPublicEDKey).toBe(PUBLIC_ED_KEY);
    expect(plist.SUEnableAutomaticChecks).toBeUndefined();
    const embedded = join(app, "Contents", "Frameworks", "Sparkle.framework");
    must(["codesign", "--verify", "--deep", "--strict", app]);
    for (const nested of [
      join(embedded, "Versions", "B", "Autoupdate"),
      join(embedded, "Versions", "B", "Updater.app"),
      embedded,
    ]) {
      const info = run(["codesign", "--display", "--verbose=2", nested]).output;
      expect(info).toContain("Signature=adhoc");
      expect(info).toMatch(/flags=0x[0-9a-f]+\([^)]*runtime[^)]*\)/);
    }
    // Ad-hoc app + ad-hoc framework: only library validation is relaxed,
    // and only on the app (the CLI never loads Sparkle).
    const ents = (path: string) =>
      run(["codesign", "--display", "--entitlements", "-", "--xml", path])
        .output;
    expect(ents(app)).toContain(
      "com.apple.security.cs.disable-library-validation",
    );
    expect(ents(join(app, "Contents", "MacOS", "polygloss-cli"))).not.toContain(
      "disable-library-validation",
    );
  });

  test("appcast settings are checked before anything is built", () => {
    const fw = makeSparkleFramework(scratch("vendor"));
    const cases: [Env, string][] = [
      [
        { POLYGLOSS_APPCAST_URL: APPCAST_URL },
        "set both POLYGLOSS_APPCAST_URL and SPARKLE_PUBLIC_ED_KEY",
      ],
      [
        { SPARKLE_PUBLIC_ED_KEY: PUBLIC_ED_KEY },
        "set both POLYGLOSS_APPCAST_URL and SPARKLE_PUBLIC_ED_KEY",
      ],
      [
        {
          POLYGLOSS_APPCAST_URL: "http://example.invalid/appcast.xml",
          SPARKLE_PUBLIC_ED_KEY: PUBLIC_ED_KEY,
        },
        "must be an https URL",
      ],
      [
        {
          POLYGLOSS_APPCAST_URL: APPCAST_URL,
          SPARKLE_PUBLIC_ED_KEY: "bm90LWEta2V5",
        },
        "not a base64 Ed25519 public key",
      ],
      [
        {
          POLYGLOSS_APPCAST_URL: APPCAST_URL,
          SPARKLE_PUBLIC_ED_KEY: PUBLIC_ED_KEY,
          POLYGLOSS_SPARKLE_FRAMEWORK: join(sandbox.home, "no-sparkle"),
        },
        "run scripts/fetch-sparkle.sh",
      ],
    ];
    for (const [env, message] of cases) {
      const r = packageRun([], { POLYGLOSS_SPARKLE_FRAMEWORK: fw, ...env });
      expect({ env, exitCode: r.exitCode, log: r.log }).toEqual({
        env,
        exitCode: 1,
        log: [],
      });
      expect(r.output).toContain(message);
    }
  });

  test("rejects unknown arguments", () => {
    const r = packageRun(["--universal"]);
    expect(r.exitCode).toBe(2);
    expect(r.log).toEqual([]);
  });
});

// The real release bundle. scripts/test-e2e.sh builds it (package-release.sh)
// and points POLYGLOSS_BUNDLE at it when POLYGLOSS_BUNDLE_E2E=1.
describe.skipIf(process.env.POLYGLOSS_BUNDLE_E2E !== "1")(
  "release bundle",
  () => {
    const app = resolve(
      repoRoot,
      process.env.POLYGLOSS_BUNDLE ?? "dist/Polygloss.app",
    );
    const macos = join(app, "Contents", "MacOS");
    let pid: number | undefined;

    afterAll(async () => {
      if (pid !== undefined) await stopAppPid(pid);
      Bun.spawnSync([lsregister, "-u", app]);
    });

    test("bundle contains Polygloss and polygloss-cli", () => {
      for (const exe of ["Polygloss", "polygloss-cli"]) {
        expect(must(["file", "-b", join(macos, exe)])).toContain(
          "Mach-O 64-bit executable arm64",
        );
      }
      expect(must([join(macos, "Polygloss"), "--version"]).trim()).toBe(
        `Polygloss ${workspaceVersion}`,
      );
      expect(must([join(macos, "polygloss-cli"), "--version"]).trim()).toBe(
        `polygloss ${workspaceVersion}`,
      );
      expect(smokeStatic(app).exitCode).toBe(0);
    });

    test("info plist has identifier url scheme and explicit version", () => {
      const plist = readPlist(join(app, "Contents", "Info.plist"));
      expect(plist).toMatchObject({
        CFBundleIdentifier: "dev.dak.polygloss",
        CFBundleExecutable: "Polygloss",
        CFBundleShortVersionString: workspaceVersion,
        CFBundleVersion: workspaceVersion,
        LSMinimumSystemVersion: "14.0",
        CFBundleIconFile: "icon.icns",
      });
      expect(
        plist.CFBundleURLTypes.flatMap(
          (t: Json) => t.CFBundleURLSchemes as string[],
        ),
      ).toEqual(["polygloss"]);
      expect(existsSync(join(app, "Contents", "Resources", "icon.icns"))).toBe(
        true,
      );
      for (const [name, src] of bundledLicenses)
        expect({
          name,
          same: readFileSync(join(app, "Contents", "Resources", name)).equals(
            readFileSync(src),
          ),
        }).toEqual({ name, same: true });
    });

    test("app icon is icon.icns, plus actool's Assets.car with Xcode 26", () => {
      const plist = readPlist(join(app, "Contents", "Info.plist"));
      const car = join(app, "Contents", "Resources", "Assets.car");
      // Xcode 26's actool is the first to compile Icon Composer documents;
      // the one package-release.sh ran.
      const developerDir = process.env.POLYGLOSS_ACTOOL_DEVELOPER_DIR;
      const xcode = /<key>short-bundle-version<\/key>\s*<string>(\d+)/.exec(
        run(["xcrun", "actool", "--version"], {
          env: developerDir ? { DEVELOPER_DIR: developerDir } : {},
        }).stdout,
      );
      if (Number(xcode?.[1] ?? 0) < 26) {
        expect(existsSync(car)).toBe(false);
        expect(plist.CFBundleIconName).toBeUndefined();
        return;
      }
      expect(plist.CFBundleIconName).toBe("polygloss");
      // assetutil reads the catalog back: the layered icon's light, dark and
      // tinted stacks, plus a flattened 1024 px rendition for macOS 14 and 15.
      const named = (
        JSON.parse(must(["xcrun", "assetutil", "--info", car])) as Json[]
      ).filter((a) => a.Name === "polygloss");
      expect(
        named
          .filter((a) => a.AssetType === "IconImageStack")
          .map((a) => a.Appearance)
          .sort(),
      ).toEqual([
        "ISAppearanceTintable",
        "NSAppearanceNameAqua",
        "NSAppearanceNameDarkAqua",
      ]);
      expect(
        named.some(
          (a) => a.AssetType === "Icon Image" && a.PixelWidth === 1024,
        ),
      ).toBe(true);
    });

    test(
      "smoke bundle passes on an unsigned local build",
      () => {
        const r = run([smokeBundle, app]);
        expect(r.output).toContain("smoke-bundle: PASS");
        expect(r.exitCode).toBe(0);
      },
      { timeout: 300_000 },
    );

    test(
      "url scheme opens a diff tab",
      async () => {
        // Short data dir: the socket path must fit in sun_path.
        const dataDir = realpathSync(
          Bun.spawnSync(["mktemp", "-d", "/tmp/pgb-XXXXXX"])
            .stdout.toString()
            .trim(),
        );
        const env: Env = {
          ...sandbox.env,
          POLYGLOSS_DATA_DIR: dataDir,
          POLYGLOSS_TEST: "1",
        };
        const socket = join(dataDir, "polygloss.sock");
        try {
          const repo = join(scratch("url-repo"), "repo");
          mkdirSync(repo);
          git(env, repo, ["init", "-q", "-b", "main"]);
          writeFileSync(join(repo, "a.txt"), "one\n");
          git(env, repo, ["add", "."]);
          git(env, repo, ["commit", "-q", "-m", "one"]);
          writeFileSync(join(repo, "a.txt"), "one\ntwo\n");
          git(env, repo, ["commit", "-q", "-am", "two"]);
          const shown = JSON.parse(
            must(
              [
                join(macos, "polygloss-cli"),
                "--json",
                "--no-open",
                "--repo",
                repo,
                "show",
                "HEAD",
              ],
              { env },
            ),
          ) as Json;
          expect(shown.diff_id).toMatch(/^[0-9a-f]{64}$/);

          // LaunchServices, not a direct exec: `open -g -a <this bundle>`.
          const envArgs = Object.entries(env).flatMap(([k, v]) => [
            "--env",
            `${k}=${v}`,
          ]);
          must(["/usr/bin/open", "-g", "-F", "-a", app, ...envArgs], { env });
          pid = await waitForApp(socket, 90_000);
          const hello = await appCall(socket, "hello", { client: "t5.1" });
          expect(hello.version).toBe(workspaceVersion);
          expect((await debugState(socket)).tabs).toEqual([]);

          must(
            [
              "/usr/bin/open",
              "-g",
              "-a",
              app,
              `polygloss://diff/${shown.diff_id}`,
            ],
            { env },
          );
          const tab = await waitForState(socket, (s) =>
            s.tabs.find((t) => t.diff_id === shown.diff_id),
          );
          expect(tab.review_id).toBe(shown.review_id);
          // The bundle points the stable CLI path at its own polygloss-cli.
          expect(realpathSync(join(dataDir, "bin", "polygloss"))).toBe(
            realpathSync(join(macos, "polygloss-cli")),
          );
        } finally {
          if (pid !== undefined) await stopAppPid(pid);
          pid = undefined;
          rmSync(dataDir, { recursive: true, force: true });
        }
      },
      { timeout: 180_000 },
    );

    test("the dmg holds the bundle", () => {
      const dmg = join(app, "..", `Polygloss_${workspaceVersion}_aarch64.dmg`);
      expect(existsSync(dmg)).toBe(true);
      const mount = scratch("mount");
      must([
        "hdiutil",
        "attach",
        "-nobrowse",
        "-readonly",
        "-noautoopen",
        "-mountpoint",
        mount,
        dmg,
      ]);
      try {
        must([
          "codesign",
          "--verify",
          "--deep",
          "--strict",
          join(mount, "Polygloss.app"),
        ]);
        expect(
          readFileSync(
            join(mount, "Polygloss.app", "Contents", "MacOS", "polygloss-cli"),
          ).equals(readFileSync(join(macos, "polygloss-cli"))),
        ).toBe(true);
      } finally {
        must(["hdiutil", "detach", "-quiet", mount]);
      }
    });
  },
);
