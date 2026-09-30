// Signing, notarization and the release workflow (plan T5.2, design §21,
// ADR-0019, library-choices §13/§14).
//
// scripts/sign-and-notarize.sh runs against fake `security`, `codesign`,
// `xcrun` and `spctl` on PATH that log their argv, so no test ever creates a
// keychain, changes the keychain search list (which lives in the real user's
// preferences, not $HOME), contacts Apple or signs with a real identity. The
// certificates are throwaway self-signed ones made with the base system's
// LibreSSL. The no-credentials case and scripts/sign-sparkle.sh's ad-hoc mode
// run the real codesign on bundles made from copies of /usr/bin/true.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

setDefaultTimeout(60_000);

type Env = Record<string, string>;

const repoRoot = resolve(import.meta.dir, "../..");
const signer = join(repoRoot, "scripts", "sign-and-notarize.sh");
const sparkleSigner = join(repoRoot, "scripts", "sign-sparkle.sh");
const releaseWorkflow = join(repoRoot, ".github", "workflows", "release.yml");
const entitlements = join(repoRoot, "packaging", "entitlements.plist");

const TEAM = "5U7E4UQ5M3";
const WRONG_TEAM = "FCSF68W94H";

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
  env: Env = {},
): { exitCode: number; output: string } {
  const r = Bun.spawnSync(argv, {
    env: { ...sandbox.env, ...env },
    cwd: sandbox.home,
  });
  return {
    exitCode: r.exitCode ?? -1,
    output: r.stdout.toString() + r.stderr.toString(),
  };
}

function must(argv: string[], env: Env = {}): string {
  const r = run(argv, env);
  if (r.exitCode !== 0)
    throw new Error(`${argv.join(" ")} failed (${r.exitCode}): ${r.output}`);
  return r.output;
}

// ---------------------------------------------------------------------------
// Fake Apple tools. Each logs `<tool> <argv>` to $FAKE_SIGN_LOG.

const fakeSecurity = `#!/bin/bash
set -euo pipefail
printf 'security %s\\n' "$*" >>"$FAKE_SIGN_LOG"
case "$1" in
  list-keychains)
    shift
    [ "$1" = -d ] && shift 2
    if [ "\${1-}" = -s ]; then
      shift
      : >"$FAKE_KEYCHAIN_LIST"
      for k in "$@"; do printf '%s\\n' "$k" >>"$FAKE_KEYCHAIN_LIST"; done
    else
      while IFS= read -r k; do printf '    "%s"\\n' "$k"; done <"$FAKE_KEYCHAIN_LIST"
    fi
    ;;
  create-keychain) : >"\${@: -1}" ;;
  delete-keychain) rm -f "\${@: -1}" ;;
esac
`;

const fakeCodesign = `#!/bin/bash
set -euo pipefail
printf 'codesign %s\\n' "$*" >>"$FAKE_SIGN_LOG"
case " $* " in *"\${FAKE_CODESIGN_FAIL:-<none>}"*) echo "fake codesign failure: $*" >&2; exit 1 ;; esac
case " $* " in *" --display "*)
  team="\${FAKE_TEAM:-${TEAM}}"
  printf 'Authority=Developer ID Application: Test Person (%s)\\nTeamIdentifier=%s\\n' "$team" "$team" >&2 ;;
esac
`;

const fakeXcrun = `#!/bin/bash
set -euo pipefail
printf 'xcrun %s\\n' "$*" >>"$FAKE_SIGN_LOG"
if [ "$1 $2" = "notarytool submit" ]; then
  printf '{"id":"fake-submission","status":"%s","message":"fake"}\\n' "\${FAKE_NOTARY_STATUS:-Accepted}"
fi
if [ "$1 $2" = "notarytool log" ]; then echo "fake notary log"; fi
`;

const fakeSpctl = `#!/bin/bash
printf 'spctl %s\\n' "$*" >>"$FAKE_SIGN_LOG"
`;

/** A PATH directory with the fake tools, plus fresh log and search-list files. */
function fakeTools(): {
  env: Env;
  log: () => string[];
  keychainList: () => string[];
  originalList: string[];
} {
  const dir = scratch("fake-tools");
  const bin = join(dir, "bin");
  mkdirSync(bin);
  for (const [name, body] of Object.entries({
    security: fakeSecurity,
    codesign: fakeCodesign,
    xcrun: fakeXcrun,
    spctl: fakeSpctl,
  })) {
    writeFileSync(join(bin, name), body);
    chmodSync(join(bin, name), 0o755);
  }
  const logPath = join(dir, "sign.log");
  const listPath = join(dir, "keychain-list");
  const originalList = [
    join(dir, "login.keychain-db"),
    "/Library/Keychains/System.keychain",
  ];
  writeFileSync(listPath, originalList.map((k) => `${k}\n`).join(""));
  const lines = (p: string) =>
    existsSync(p) ? readFileSync(p, "utf8").split("\n").filter(Boolean) : [];
  return {
    env: {
      PATH: `${bin}:${sandbox.env.PATH ?? "/usr/bin:/bin"}`,
      FAKE_SIGN_LOG: logPath,
      FAKE_KEYCHAIN_LIST: listPath,
    },
    log: () => lines(logPath),
    keychainList: () => lines(listPath),
    originalList,
  };
}

// ---------------------------------------------------------------------------
// Throwaway certificates.

/** A self-signed code-signing identity as APPLE_CERTIFICATE (base64 .p12). */
function makeCertificate(opts: { cn: string; ou: string; password?: string }): {
  base64: string;
  password: string;
  sha1: string;
} {
  const dir = scratch("cert");
  const password = opts.password ?? "p12-password";
  must([
    "/usr/bin/openssl",
    "req",
    "-x509",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-days",
    "2",
    "-keyout",
    join(dir, "key.pem"),
    "-out",
    join(dir, "cert.pem"),
    "-subj",
    `/UID=${opts.ou}/CN=${opts.cn}/OU=${opts.ou}/O=Test Person/C=US`,
    "-addext",
    "extendedKeyUsage=codeSigning",
  ]);
  must([
    "/usr/bin/openssl",
    "pkcs12",
    "-export",
    "-inkey",
    join(dir, "key.pem"),
    "-in",
    join(dir, "cert.pem"),
    "-out",
    join(dir, "cert.p12"),
    "-passout",
    `pass:${password}`,
  ]);
  const sha1 = must([
    "/usr/bin/openssl",
    "x509",
    "-in",
    join(dir, "cert.pem"),
    "-noout",
    "-fingerprint",
    "-sha1",
  ])
    .trim()
    .replace(/^.*=/, "")
    .replaceAll(":", "");
  return {
    base64: readFileSync(join(dir, "cert.p12")).toString("base64"),
    password,
    sha1,
  };
}

const goodName = `Developer ID Application: Test Person (${TEAM})`;

// ---------------------------------------------------------------------------
// Bundles.

function writePlist(path: string, value: Record<string, string>): void {
  writeFileSync(`${path}.json`, JSON.stringify(value));
  must(["plutil", "-convert", "xml1", "-o", path, `${path}.json`]);
  must(["rm", `${path}.json`]);
}

function machO(path: string): void {
  copyFileSync("/usr/bin/true", path);
  chmodSync(path, 0o755);
}

/** Sparkle.framework's layout (library-choices §14), Mach-O stand-ins. */
function makeSparkle(frameworks: string, opts: { xpc?: boolean } = {}): string {
  const fw = join(frameworks, "Sparkle.framework");
  const b = join(fw, "Versions", "B");
  mkdirSync(join(b, "Resources"), { recursive: true });
  machO(join(b, "Sparkle"));
  machO(join(b, "Autoupdate"));
  writePlist(join(b, "Resources", "Info.plist"), {
    CFBundleExecutable: "Sparkle",
    CFBundleIdentifier: "org.sparkle-project.Sparkle",
    CFBundlePackageType: "FMWK",
  });
  const bundles: [string, string][] = [["Updater.app", "Updater"]];
  if (opts.xpc)
    bundles.push(
      ["XPCServices/Installer.xpc", "Installer"],
      ["XPCServices/Downloader.xpc", "Downloader"],
    );
  for (const [rel, exe] of bundles) {
    const contents = join(b, rel, "Contents");
    mkdirSync(join(contents, "MacOS"), { recursive: true });
    machO(join(contents, "MacOS", exe));
    writePlist(join(contents, "Info.plist"), {
      CFBundleExecutable: exe,
      CFBundleIdentifier: `org.sparkle-project.${exe}`,
    });
  }
  symlinkSync("B", join(fw, "Versions", "Current"));
  symlinkSync("Versions/Current/Sparkle", join(fw, "Sparkle"));
  symlinkSync("Versions/Current/Resources", join(fw, "Resources"));
  return fw;
}

/** Polygloss.app as package-release.sh hands it over (ad-hoc signed). */
function makeApp(opts: { sparkle?: boolean; sign?: boolean } = {}): string {
  const app = join(scratch("bundle"), "Polygloss.app");
  mkdirSync(join(app, "Contents", "MacOS"), { recursive: true });
  machO(join(app, "Contents", "MacOS", "Polygloss"));
  machO(join(app, "Contents", "MacOS", "polygloss-cli"));
  writePlist(join(app, "Contents", "Info.plist"), {
    CFBundleExecutable: "Polygloss",
    CFBundleIdentifier: "dev.dak.polygloss",
    CFBundlePackageType: "APPL",
  });
  if (opts.sparkle)
    makeSparkle(join(app, "Contents", "Frameworks"), { xpc: false });
  if (opts.sign !== false) {
    must([
      "/usr/bin/codesign",
      "--force",
      "--sign",
      "-",
      "--identifier",
      "dev.dak.polygloss.cli",
      join(app, "Contents", "MacOS", "polygloss-cli"),
    ]);
    must(["/usr/bin/codesign", "--force", "--sign", "-", app]);
  }
  return app;
}

function makeDmg(): string {
  const dmg = join(scratch("dmg"), "Polygloss_0.1.0_aarch64.dmg");
  writeFileSync(dmg, "not really a disk image");
  return dmg;
}

/** The real signature of `path`: CDHash and team, from the real codesign. */
function realSignature(path: string): string {
  return must(["/usr/bin/codesign", "--display", "--verbose=4", path])
    .split("\n")
    .filter((l) => /^(CDHash|Signature|TeamIdentifier|Identifier)=/.test(l))
    .join("\n");
}

const notaryApiEnv = (): Env => {
  const key = join(scratch("api-key"), "AuthKey_TESTKEY.p8");
  writeFileSync(key, "-----BEGIN PRIVATE KEY-----\nfake\n");
  return {
    APPLE_API_KEY: "TESTKEY",
    APPLE_API_ISSUER: "00000000-0000-0000-0000-000000000000",
    APPLE_API_KEY_PATH: key,
  };
};

/** The paths codesign signed (not verified or displayed), in order. */
function signedPaths(log: string[]): string[] {
  return log
    .filter((l) => l.startsWith("codesign ") && l.includes("--sign "))
    .map((l) => l.split(" ").at(-1) ?? "");
}

// ---------------------------------------------------------------------------

describe("scripts/sign-and-notarize.sh", () => {
  test("skips cleanly without credentials", () => {
    const tools = fakeTools();
    const app = makeApp();
    const before = realSignature(app);
    const dmg = makeDmg();
    const dmgBytes = readFileSync(dmg);

    for (const extra of [
      {},
      // GitHub Actions passes an empty string for a secret that is not set.
      { APPLE_CERTIFICATE: "", APPLE_SIGNING_IDENTITY: "" },
      // Notarization credentials alone sign nothing.
      notaryApiEnv(),
      { APPLE_KEYCHAIN_PROFILE: "polygloss-notary" },
    ]) {
      for (const target of [app, dmg]) {
        const r = run([signer, target], { ...tools.env, ...extra });
        expect(r.output).toContain("signing skipped: no credentials");
        expect(r.output).toContain(target);
        expect(r.exitCode).toBe(0);
      }
    }
    // No tool ran: the ad-hoc signature is left exactly as it was.
    expect(tools.log()).toEqual([]);
    expect(tools.keychainList()).toEqual(tools.originalList);
    expect(realSignature(app)).toBe(before);
    must(["/usr/bin/codesign", "--verify", "--deep", "--strict", app]);
    expect(readFileSync(dmg).equals(dmgBytes)).toBe(true);
  });

  test("refuses team FCSF68W94H identity", () => {
    const tools = fakeTools();
    const app = makeApp();
    const before = realSignature(app);
    const wrongName = `Developer ID Application: Someone Else (${WRONG_TEAM})`;
    const wrongCert = makeCertificate({ cn: wrongName, ou: WRONG_TEAM });
    // The certificate's OU is the team, whatever its name claims.
    const disguised = makeCertificate({ cn: goodName, ou: WRONG_TEAM });

    const cases: Env[] = [
      { APPLE_SIGNING_IDENTITY: wrongName },
      {
        APPLE_CERTIFICATE: wrongCert.base64,
        APPLE_CERTIFICATE_PASSWORD: wrongCert.password,
      },
      {
        APPLE_CERTIFICATE: disguised.base64,
        APPLE_CERTIFICATE_PASSWORD: disguised.password,
      },
      {
        APPLE_CERTIFICATE: wrongCert.base64,
        APPLE_CERTIFICATE_PASSWORD: wrongCert.password,
        APPLE_SIGNING_IDENTITY: goodName,
      },
    ];
    for (const env of cases) {
      const r = run([signer, app], { ...tools.env, ...env });
      expect({ env: Object.keys(env), exitCode: r.exitCode }).toEqual({
        env: Object.keys(env),
        exitCode: 1,
      });
      expect(r.output).toContain(`refusing team ${WRONG_TEAM}`);
    }
    // Refused before any keychain, codesign or notary call.
    expect(tools.log()).toEqual([]);
    expect(tools.keychainList()).toEqual(tools.originalList);
    expect(realSignature(app)).toBe(before);
  });

  test("refuses identity without team 5U7E4UQ5M3", () => {
    const tools = fakeTools();
    const app = makeApp();
    const other = makeCertificate({
      cn: "Developer ID Application: Other Person (ABCDE12345)",
      ou: "ABCDE12345",
    });
    const good = makeCertificate({ cn: goodName, ou: TEAM });

    const cases: [Env, string][] = [
      [
        {
          APPLE_SIGNING_IDENTITY:
            "Developer ID Application: Other Person (ABCDE12345)",
        },
        "ABCDE12345",
      ],
      [
        { APPLE_SIGNING_IDENTITY: "Developer ID Application: Test Person" },
        "no team",
      ],
      // A SHA-1 hash names no team; only a certificate can vouch for it.
      [{ APPLE_SIGNING_IDENTITY: good.sha1 }, "no team"],
      // The one 5U7E4UQ5M3 certificate on the dev machine cannot notarize.
      [
        { APPLE_SIGNING_IDENTITY: `Apple Distribution: Test Person (${TEAM})` },
        "Developer ID Application",
      ],
      [
        {
          APPLE_CERTIFICATE: other.base64,
          APPLE_CERTIFICATE_PASSWORD: other.password,
        },
        "ABCDE12345",
      ],
      // The identity must name the certificate's own identity.
      [
        {
          APPLE_CERTIFICATE: good.base64,
          APPLE_CERTIFICATE_PASSWORD: good.password,
          APPLE_SIGNING_IDENTITY: `Developer ID Application: Somebody (${TEAM})`,
        },
        "does not match",
      ],
      [
        {
          APPLE_CERTIFICATE: good.base64,
          APPLE_CERTIFICATE_PASSWORD: "wrong password",
        },
        "cannot read APPLE_CERTIFICATE",
      ],
      [
        {
          APPLE_CERTIFICATE: "bm90IGEgcDEy",
          APPLE_CERTIFICATE_PASSWORD: "x",
        },
        "cannot read APPLE_CERTIFICATE",
      ],
    ];
    for (const [env, reason] of cases) {
      const r = run([signer, app], { ...tools.env, ...env });
      expect({ env, exitCode: r.exitCode }).toEqual({ env, exitCode: 1 });
      expect({ env, reason, output: r.output.includes(reason) }).toEqual({
        env,
        reason,
        output: true,
      });
    }
    expect(tools.log()).toEqual([]);
    expect(tools.keychainList()).toEqual(tools.originalList);
  });

  test("signs an app inside out in a temporary keychain, notarizes, staples and verifies", () => {
    const tools = fakeTools();
    // Unsigned: the fake codesign never reads it.
    const app = makeApp({ sparkle: true, sign: false });
    const cert = makeCertificate({ cn: goodName, ou: TEAM });
    const r = run([signer, app], {
      ...tools.env,
      ...notaryApiEnv(),
      APPLE_CERTIFICATE: cert.base64,
      APPLE_CERTIFICATE_PASSWORD: cert.password,
    });
    expect(r.output).toContain("sign-and-notarize: done");
    expect(r.exitCode).toBe(0);
    const log = tools.log();

    // The temporary keychain: created outside ~/Library, searched only while
    // signing, then deleted; the user's search list is restored exactly.
    const create = log.find((l) => l.startsWith("security create-keychain"));
    const keychain = create?.split(" ").at(-1) ?? "";
    expect(keychain).toEndWith(".keychain-db");
    expect(keychain.startsWith(sandbox.env.TMPDIR ?? "/")).toBe(true);
    expect(keychain).not.toContain("/Library/");
    expect(log).toContain(
      `security list-keychains -d user -s ${keychain} ${tools.originalList.join(" ")}`,
    );
    expect(log.at(-2)).toBe(
      `security list-keychains -d user -s ${tools.originalList.join(" ")}`,
    );
    expect(log.at(-1)).toBe(`security delete-keychain ${keychain}`);
    expect(existsSync(keychain)).toBe(false);
    expect(tools.keychainList()).toEqual(tools.originalList);
    expect(log.some((l) => /^security import \S+ -k \S+ /.test(l))).toBe(true);

    // Inside out: Sparkle's helpers, the framework, both executables, the app.
    const fw = join(app, "Contents", "Frameworks", "Sparkle.framework");
    expect(signedPaths(log)).toEqual([
      join(fw, "Versions", "B", "Autoupdate"),
      join(fw, "Versions", "B", "Updater.app"),
      fw,
      join(app, "Contents", "MacOS", "polygloss-cli"),
      join(app, "Contents", "MacOS", "Polygloss"),
      app,
    ]);
    for (const line of log.filter(
      (l) => l.startsWith("codesign ") && l.includes("--sign "),
    )) {
      expect(line).toContain(`--sign ${cert.sha1}`);
      expect(line).toContain("--options runtime");
      expect(line).toContain("--timestamp");
      expect(line).not.toContain("--timestamp=none");
      expect(line).toContain(`--keychain ${keychain}`);
    }
    const signLine = (path: string) =>
      log.find((l) => l.includes("--sign ") && l.endsWith(` ${path}`)) ?? "";
    for (const [exe, id] of [
      ["polygloss-cli", "dev.dak.polygloss.cli"],
      ["Polygloss", "dev.dak.polygloss"],
    ] as const) {
      const line = signLine(join(app, "Contents", "MacOS", exe));
      expect(line).toContain(`--identifier ${id}`);
      expect(line).toContain(`--entitlements ${entitlements}`);
    }
    expect(signLine(app)).toContain(`--entitlements ${entitlements}`);
    expect(signLine(fw)).not.toContain("--entitlements");

    // Verify, notarize the zipped app, staple, assess; all after signing.
    const after = log.slice(log.indexOf(signLine(app)) + 1);
    const steps = after
      .filter((l) => !l.startsWith("security "))
      .map((l) => l.split(" ").slice(0, 3).join(" "));
    expect(steps).toEqual([
      "codesign --verify --deep",
      "codesign --display --verbose=2",
      "xcrun notarytool submit",
      "xcrun stapler staple",
      "xcrun stapler validate",
      "spctl --assess --type",
    ]);
    const submit = after.find((l) => l.startsWith("xcrun notarytool submit"));
    expect(submit).toMatch(/submit \S+\.zip /);
    expect(submit).toContain("--wait");
    expect(submit).toContain("--key-id TESTKEY");
    expect(submit).toContain("--issuer 00000000-0000-0000-0000-000000000000");
    expect(submit).toContain("--key ");
    expect(after).toContain(`xcrun stapler staple ${app}`);
    expect(after).toContain(`spctl --assess --type execute -vv ${app}`);
  });

  test("signs, notarizes and staples a dmg with a keychain profile", () => {
    const tools = fakeTools();
    const dmg = makeDmg();
    const cert = makeCertificate({ cn: goodName, ou: TEAM });
    const r = run([signer, dmg], {
      ...tools.env,
      APPLE_CERTIFICATE: cert.base64,
      APPLE_CERTIFICATE_PASSWORD: cert.password,
      APPLE_SIGNING_IDENTITY: goodName,
      APPLE_KEYCHAIN_PROFILE: "polygloss-notary",
    });
    expect(r.output).toContain("sign-and-notarize: done");
    expect(r.exitCode).toBe(0);
    const steps = tools
      .log()
      .filter((l) => !l.startsWith("security "))
      .map((l) => l.replace(/ --keychain \S+/, ""));
    expect(steps).toEqual([
      `codesign --force --sign ${cert.sha1} --timestamp ${dmg}`,
      `codesign --verify --deep --strict --verbose=2 ${dmg}`,
      `codesign --display --verbose=2 ${dmg}`,
      `xcrun notarytool submit ${dmg} --wait --output-format json --keychain-profile polygloss-notary`,
      `xcrun stapler staple ${dmg}`,
      `xcrun stapler validate ${dmg}`,
      `spctl --assess --type open --context context:primary-signature -vv ${dmg}`,
    ]);
    expect(tools.keychainList()).toEqual(tools.originalList);
  });

  test("an identity already in the user's keychains needs no temporary keychain", () => {
    const tools = fakeTools();
    const app = makeApp();
    const r = run([signer, app], {
      ...tools.env,
      APPLE_SIGNING_IDENTITY: goodName,
      APPLE_KEYCHAIN_PROFILE: "polygloss-notary",
    });
    expect(r.exitCode).toBe(0);
    const log = tools.log();
    expect(log.filter((l) => l.startsWith("security "))).toEqual([]);
    const signs = log.filter((l) => l.includes("--sign "));
    expect(signs.length).toBe(3);
    for (const l of signs) {
      expect(l).toContain(`--sign ${goodName}`);
      expect(l).not.toContain("--keychain");
    }
  });

  test("restores the keychain search list and deletes the keychain when signing fails", () => {
    const tools = fakeTools();
    const app = makeApp();
    const cert = makeCertificate({ cn: goodName, ou: TEAM });
    const r = run([signer, app], {
      ...tools.env,
      APPLE_CERTIFICATE: cert.base64,
      APPLE_CERTIFICATE_PASSWORD: cert.password,
      APPLE_KEYCHAIN_PROFILE: "polygloss-notary",
      FAKE_CODESIGN_FAIL: "polygloss-cli",
    });
    expect(r.exitCode).not.toBe(0);
    expect(r.output).toContain("fake codesign failure");
    const log = tools.log();
    const keychain =
      log
        .find((l) => l.startsWith("security create-keychain"))
        ?.split(" ")
        .at(-1) ?? "<none>";
    expect(log.at(-1)).toBe(`security delete-keychain ${keychain}`);
    expect(existsSync(keychain)).toBe(false);
    expect(tools.keychainList()).toEqual(tools.originalList);
    expect(log.some((l) => l.startsWith("xcrun "))).toBe(false);
  });

  test("fails when the signature does not carry team 5U7E4UQ5M3", () => {
    const tools = fakeTools();
    const r = run([signer, makeApp()], {
      ...tools.env,
      APPLE_SIGNING_IDENTITY: goodName,
      APPLE_KEYCHAIN_PROFILE: "polygloss-notary",
      FAKE_TEAM: "ABCDE12345",
    });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("TeamIdentifier");
    expect(tools.log().some((l) => l.startsWith("xcrun "))).toBe(false);
  });

  test("fails with the notary log when notarization is not accepted", () => {
    const tools = fakeTools();
    const dmg = makeDmg();
    const r = run([signer, dmg], {
      ...tools.env,
      APPLE_SIGNING_IDENTITY: goodName,
      APPLE_KEYCHAIN_PROFILE: "polygloss-notary",
      FAKE_NOTARY_STATUS: "Invalid",
    });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("Invalid");
    expect(r.output).toContain("fake notary log");
    const log = tools.log();
    expect(log).toContain(
      "xcrun notarytool log fake-submission --keychain-profile polygloss-notary",
    );
    expect(log.some((l) => l.startsWith("xcrun stapler"))).toBe(false);
  });

  test("signs without notarytool credentials, unless notarization is required", () => {
    const tools = fakeTools();
    const app = makeApp();
    const env = { ...tools.env, APPLE_SIGNING_IDENTITY: goodName };
    const r = run([signer, app], env);
    expect(r.exitCode).toBe(0);
    expect(r.output).toContain(
      "notarization skipped: no notarytool credentials",
    );
    expect(tools.log().some((l) => l.startsWith("xcrun "))).toBe(false);
    // A Developer ID signature without a ticket fails Gatekeeper: not assessed.
    expect(tools.log().some((l) => l.startsWith("spctl "))).toBe(false);
    expect(signedPaths(tools.log()).at(-1)).toBe(app);

    const required = fakeTools();
    const strict = run([signer, app], {
      ...required.env,
      APPLE_SIGNING_IDENTITY: goodName,
      POLYGLOSS_REQUIRE_NOTARIZATION: "1",
    });
    expect(strict.exitCode).toBe(1);
    expect(strict.output).toContain("notarytool credentials");
    // Checked before anything is signed.
    expect(required.log()).toEqual([]);
  });

  test("incomplete API key credentials are an error", () => {
    const tools = fakeTools();
    const { APPLE_API_KEY_PATH: _, ...partial } = notaryApiEnv();
    const r = run([signer, makeApp()], {
      ...tools.env,
      ...partial,
      APPLE_SIGNING_IDENTITY: goodName,
    });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("APPLE_API_KEY_PATH");
    expect(tools.log()).toEqual([]);
  });

  test("usage errors", () => {
    const tools = fakeTools();
    expect(run([signer], tools.env).exitCode).toBe(2);
    expect(run([signer, "a.app", "b.dmg"], tools.env).exitCode).toBe(2);
    const zip = join(scratch("zip"), "Polygloss.zip");
    writeFileSync(zip, "");
    expect(run([signer, zip], tools.env).exitCode).toBe(2);
    const missing = run([signer, join(sandbox.home, "none.app")], tools.env);
    expect(missing.exitCode).toBe(1);
    expect(missing.output).toContain("none.app");
    expect(tools.log()).toEqual([]);
  });
});

describe("scripts/sign-sparkle.sh", () => {
  test("signs Sparkle's nested code inside out, then the framework", () => {
    const tools = fakeTools();
    const fw = makeSparkle(scratch("frameworks"), { xpc: true });
    const b = join(fw, "Versions", "B");
    must([sparkleSigner, fw, goodName, "--keychain", "/tmp/k.keychain-db"], {
      ...tools.env,
    });
    const log = tools.log();
    expect(signedPaths(log)).toEqual([
      join(b, "XPCServices", "Installer.xpc"),
      join(b, "XPCServices", "Downloader.xpc"),
      join(b, "Autoupdate"),
      join(b, "Updater.app"),
      fw,
    ]);
    for (const l of log) {
      expect(l).toContain(`--sign ${goodName}`);
      expect(l).toContain("--options runtime");
      expect(l).toMatch(/--timestamp( |$)/);
      expect(l).toContain("--keychain /tmp/k.keychain-db");
    }
    // Sparkle's docs keep the Downloader's own entitlements.
    expect(log[1]).toContain("--preserve-metadata=entitlements");
    expect(log.filter((l) => l.includes("--preserve-metadata")).length).toBe(1);
  });

  test("ad-hoc signs a framework that verifies deep and strict", () => {
    const fw = makeSparkle(scratch("frameworks"));
    must([sparkleSigner, fw, "-"]);
    must(["/usr/bin/codesign", "--verify", "--deep", "--strict", fw]);
    for (const nested of ["Autoupdate", "Updater.app"]) {
      const info = must([
        "/usr/bin/codesign",
        "--display",
        "--verbose=2",
        join(fw, "Versions", "B", nested),
      ]);
      expect(info).toContain("Signature=adhoc");
      expect(info).toMatch(/flags=0x[0-9a-f]+\([^)]*runtime[^)]*\)/);
    }
  });

  test("usage errors", () => {
    expect(run([sparkleSigner]).exitCode).toBe(2);
    expect(
      run([sparkleSigner, join(sandbox.home, "x.framework")]).exitCode,
    ).toBe(2);
    const notFw = scratch("not-a-framework");
    expect(run([sparkleSigner, notFw, "-"]).exitCode).toBe(1);
  });
});

// ---------------------------------------------------------------------------

type Step = {
  name?: string;
  uses?: string;
  run?: string;
  if?: string;
  env?: Record<string, string>;
  with?: Record<string, unknown>;
};
type Job = {
  "runs-on"?: string;
  if?: string;
  env?: Record<string, string>;
  permissions?: Record<string, string>;
  steps?: Step[];
};
type Workflow = {
  on?: Record<string, any>;
  env?: Record<string, string>;
  permissions?: Record<string, string>;
  jobs?: Record<string, Job>;
};

let release: { wf: Workflow; text: string } | undefined;
/** release.yml, parsed once so steps compare by identity. */
function loadRelease(): { wf: Workflow; text: string } {
  if (!release) {
    const text = readFileSync(releaseWorkflow, "utf8");
    release = { wf: Bun.YAML.parse(text) as Workflow, text };
  }
  return release;
}

function releaseJob(): Job {
  const job = loadRelease().wf.jobs?.release;
  if (!job) throw new Error("release.yml has no release job");
  return job;
}

function stepRunning(needle: string): Step {
  const step = (releaseJob().steps ?? []).find((s) => s.run?.includes(needle));
  if (!step) throw new Error(`release.yml has no step running ${needle}`);
  return step;
}

describe("release workflow", () => {
  test("release workflow reads secrets only from env", () => {
    const { wf, text } = loadRelease();
    const envMaps: Record<string, string>[] = [];
    if (wf.env) envMaps.push(wf.env);
    for (const job of Object.values(wf.jobs ?? {})) {
      if (job.env) envMaps.push(job.env);
      expect(JSON.stringify(job.if ?? "")).not.toContain("secrets.");
      for (const step of job.steps ?? []) {
        if (step.env) envMaps.push(step.env);
        for (const field of [step.run, step.if, step.name, step.with]) {
          const text = JSON.stringify(field ?? "");
          expect({
            step: step.name,
            secret: text.includes("secrets."),
          }).toEqual({ step: step.name, secret: false });
        }
      }
    }
    // Every secret is one whole env value named after itself.
    let fromEnv = 0;
    for (const env of envMaps) {
      for (const [key, value] of Object.entries(env)) {
        if (!String(value).includes("secrets.")) continue;
        fromEnv += 1;
        expect({ key, value }).toEqual({
          key,
          value: `\${{ secrets.${key} }}`,
        });
      }
    }
    expect(fromEnv).toBeGreaterThan(0);
    // …and nothing else in the file mentions a secret.
    expect(text.match(/secrets\./g)?.length).toBe(fromEnv);
  });

  test("runs on v* tags and on demand, on macOS arm64", () => {
    const { wf } = loadRelease();
    expect(wf.on?.push?.tags).toEqual(["v*"]);
    expect(wf.on?.push?.branches).toBeUndefined();
    expect(Object.keys(wf.on ?? {})).toContain("workflow_dispatch");
    expect(wf.permissions).toEqual({ contents: "read" });
    const job = releaseJob();
    expect(job["runs-on"]).toBe("macos-15");
    expect(job.permissions).toEqual({ contents: "write" });
  });

  test("builds, signs and notarizes through package-release.sh --sign", () => {
    const pkg = stepRunning("scripts/package-release.sh");
    expect(pkg.run).toContain("scripts/package-release.sh --sign");
    for (const key of [
      "APPLE_CERTIFICATE",
      "APPLE_CERTIFICATE_PASSWORD",
      "APPLE_SIGNING_IDENTITY",
      "APPLE_API_KEY",
      "APPLE_API_ISSUER",
    ])
      expect(pkg.env?.[key]).toBe(`\${{ secrets.${key} }}`);
    expect(pkg.env?.POLYGLOSS_REQUIRE_NOTARIZATION).toBe("1");
    // The .p8 key is a secret written to a runner temp file first.
    const key = stepRunning("APPLE_API_KEY_PATH=");
    expect(key.env?.APPLE_API_PRIVATE_KEY).toBe(
      "${{ secrets.APPLE_API_PRIVATE_KEY }}",
    );
    expect(key.run).toContain("$RUNNER_TEMP");
    expect(key.run).toContain("$GITHUB_ENV");
    const steps = releaseJob().steps ?? [];
    expect(steps.indexOf(key)).toBeLessThan(steps.indexOf(pkg));
    // cargo-packager comes from crates.io through the cargo wrapper.
    expect(stepRunning("cargo-packager").run).toContain(
      "scripts/cargo.sh install cargo-packager --version =0.11.8 --locked",
    );
  });

  test("uploads with gh release upload only for a tag, then the appcast and tap", () => {
    const steps = releaseJob().steps ?? [];
    const upload = stepRunning("gh release upload");
    expect(upload.if).toContain("startsWith(github.ref, 'refs/tags/v')");
    expect(upload.env?.GH_TOKEN).toBe("${{ github.token }}");
    const appcast = stepRunning("make-appcast.sh");
    const tap = stepRunning("bump-tap.ts");
    expect(appcast.env?.SPARKLE_PRIVATE_ED_KEY).toBe(
      "${{ secrets.SPARKLE_PRIVATE_ED_KEY }}",
    );
    expect(tap.env?.HOMEBREW_TAP_TOKEN).toBe(
      "${{ secrets.HOMEBREW_TAP_TOKEN }}",
    );
    for (const s of [appcast, tap])
      expect(s.if).toContain("startsWith(github.ref, 'refs/tags/v')");
    const pkg = stepRunning("scripts/package-release.sh");
    expect(steps.indexOf(pkg)).toBeLessThan(steps.indexOf(upload));
    expect(steps.indexOf(upload)).toBeLessThan(steps.indexOf(appcast));
    expect(steps.indexOf(appcast)).toBeLessThan(steps.indexOf(tap));
    // A dry run (workflow_dispatch) keeps the DMG as a run artifact instead.
    expect(
      steps.some((s) => s.uses?.startsWith("actions/upload-artifact@")),
    ).toBe(true);
  });

  test("checks the tag against the crate version", () => {
    const check = stepRunning("GITHUB_REF_NAME");
    expect(check.if).toContain("startsWith(github.ref, 'refs/tags/v')");
    expect(check.run).toContain("Cargo.toml");
  });

  test("never calls bare cargo or names team FCSF68W94H", () => {
    const { text } = loadRelease();
    expect(text).not.toContain(WRONG_TEAM);
    for (const step of releaseJob().steps ?? []) {
      for (const line of (step.run ?? "").split("\n")) {
        expect({ line, bare: /(^|[\s;&|(])cargo\s/.test(line) }).toEqual({
          line,
          bare: false,
        });
      }
    }
  });
});

describe("repo hygiene for signing", () => {
  test("no signing script names team FCSF68W94H except to refuse it", () => {
    for (const script of [signer, sparkleSigner]) {
      const text = readFileSync(script, "utf8");
      const mentions = text.split("\n").filter((l) => l.includes(WRONG_TEAM));
      for (const l of mentions) expect(l).toMatch(/refus|reject|never/i);
    }
    expect(readdirSync(join(repoRoot, "scripts"))).toContain(
      basename(sparkleSigner),
    );
  });
});
