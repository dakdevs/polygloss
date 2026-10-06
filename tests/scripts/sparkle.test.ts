// Sparkle packaging scripts (plan T5.3, library-choices §14, OQ-16):
// scripts/fetch-sparkle.sh against a fake `curl` that serves a stand-in
// release (no network), scripts/make-appcast.sh against a fake
// `generate_appcast`, and the Sparkle steps of release.yml. The bundle side
// (package-release.sh embedding the framework, the smoke checks) is in
// package.test.ts.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";
import { makeSparkleFramework } from "../support/sparkle-fixture";

type Env = Record<string, string>;

const repoRoot = resolve(import.meta.dir, "../..");
const fetchSparkle = join(repoRoot, "scripts", "fetch-sparkle.sh");
const makeAppcast = join(repoRoot, "scripts", "make-appcast.sh");
const releaseWorkflow = join(repoRoot, ".github", "workflows", "release.yml");

const VERSION = "2.10.0";
const ASSET = `Sparkle-${VERSION}.tar.xz`;
const API_URL = `https://api.github.com/repos/sparkle-project/Sparkle/releases/tags/${VERSION}`;
const DOWNLOAD_URL = `https://github.com/sparkle-project/Sparkle/releases/download/${VERSION}/${ASSET}`;
/** The SHA-256 GitHub publishes for the real asset (checked 2026-09-30). */
const REAL_DIGEST =
  "c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c";

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

let scratchCount = 0;
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

function sha256(path: string): string {
  return new Bun.CryptoHasher("sha256")
    .update(readFileSync(path))
    .digest("hex");
}

function writeExecutable(path: string, text: string): void {
  writeFileSync(path, text);
  chmodSync(path, 0o755);
}

function lines(path: string): string[] {
  return existsSync(path)
    ? readFileSync(path, "utf8").split("\n").filter(Boolean)
    : [];
}

// A fake curl: logs its whole argv (one line) and the URL, then serves
// $FAKE_CURL_DIR/api.json for the release API and $FAKE_CURL_DIR/<asset> for
// the download; anything else is a 404 (curl -f exits 22).
const fakeCurl = `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$FAKE_CURL_ARGV"
out=""
url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    -H|--proto|--retry) shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
printf '%s\\n' "$url" >>"$FAKE_CURL_LOG"
case "$url" in
  "${API_URL}") src="$FAKE_CURL_DIR/api.json" ;;
  "${DOWNLOAD_URL}") src="$FAKE_CURL_DIR/${ASSET}" ;;
  *) echo "curl: (22) 404 $url" >&2; exit 22 ;;
esac
cp "$src" "$out"
`;

describe("scripts/fetch-sparkle.sh", () => {
  let release = ""; // the stand-in release tarball
  let releaseDigest = "";
  let fakeBin = "";

  beforeAll(() => {
    // The release tarball's layout: ./Sparkle.framework (with its XPC
    // services), ./bin/<tools>, and more we do not use.
    const root = scratch("release");
    makeSparkleFramework(root, { xpc: true });
    mkdirSync(join(root, "bin", "old_dsa_scripts"), { recursive: true });
    for (const tool of [
      "generate_appcast",
      "sign_update",
      "generate_keys",
      "BinaryDelta",
    ])
      writeExecutable(join(root, "bin", tool), "#!/bin/sh\nexit 0\n");
    writeFileSync(join(root, "LICENSE"), "MIT\n");
    const out = scratch("release-tar");
    release = join(out, ASSET);
    must(["tar", "-cJf", release, "-C", root, "."]);
    releaseDigest = sha256(release);
    fakeBin = scratch("fake-bin");
    writeExecutable(join(fakeBin, "curl"), fakeCurl);
  });

  /** A server dir with the API answer and the asset (optionally tampered). */
  function serve(opts: { digest?: string | null; tamper?: boolean } = {}) {
    const dir = scratch("serve");
    const digest =
      opts.digest === undefined ? `sha256:${releaseDigest}` : opts.digest;
    const assets = [
      {
        name: "Sparkle-for-Swift-Package-Manager.zip",
        digest: `sha256:${"0".repeat(64)}`,
        browser_download_url: `https://github.com/sparkle-project/Sparkle/releases/download/${VERSION}/Sparkle-for-Swift-Package-Manager.zip`,
      },
      {
        name: ASSET,
        ...(digest === null ? {} : { digest }),
        browser_download_url: DOWNLOAD_URL,
      },
    ];
    writeFileSync(
      join(dir, "api.json"),
      JSON.stringify({ tag_name: VERSION, label: null, assets }),
    );
    const bytes = readFileSync(release);
    if (opts.tamper) bytes[bytes.length - 1] = bytes[bytes.length - 1]! ^ 0xff;
    writeFileSync(join(dir, ASSET), bytes);
    return dir;
  }

  function fetchRun(
    args: string[] = [],
    opts: { serveDir?: string; vendor?: string; env?: Env } = {},
  ) {
    const vendor = opts.vendor ?? join(scratch("vendor"), "vendor");
    const logs = scratch("curl-log");
    const r = run([fetchSparkle, ...args], {
      PATH: `${fakeBin}:${sandbox.env.PATH}`,
      POLYGLOSS_VENDOR_DIR: vendor,
      FAKE_CURL_DIR: opts.serveDir ?? serve(),
      FAKE_CURL_LOG: join(logs, "urls"),
      FAKE_CURL_ARGV: join(logs, "argv"),
      SPARKLE_SHA256: releaseDigest,
      ...opts.env,
    });
    return {
      ...r,
      vendor,
      urls: lines(join(logs, "urls")),
      argv: lines(join(logs, "argv")),
    };
  }

  test("installs the framework without its XPC services, and the tools", () => {
    const r = fetchRun();
    expect(r.output).toContain(`fetch-sparkle: Sparkle ${VERSION} in`);
    expect(r.exitCode).toBe(0);
    expect(r.urls).toEqual([API_URL, DOWNLOAD_URL]);
    const fw = join(r.vendor, "Sparkle.framework");
    expect(existsSync(join(fw, "Versions", "B", "Sparkle"))).toBe(true);
    expect(existsSync(join(fw, "Versions", "B", "Autoupdate"))).toBe(true);
    // The XPC services are for sandboxed apps only; the top-level link goes
    // too, so nothing dangles.
    expect(existsSync(join(fw, "Versions", "B", "XPCServices"))).toBe(false);
    expect(() => lstatSync(join(fw, "XPCServices"))).toThrow();
    for (const tool of ["generate_appcast", "sign_update", "generate_keys"])
      expect(
        lstatSync(join(r.vendor, "sparkle-bin", tool)).mode & 0o111,
      ).not.toBe(0);
    expect(readFileSync(join(r.vendor, "sparkle.version"), "utf8")).toBe(
      `${VERSION} sha256:${releaseDigest}\n`,
    );
    // What package-release.sh does with it: sign it ad-hoc, inside out.
    must([join(repoRoot, "scripts", "sign-sparkle.sh"), fw, "-"]);
    must(["codesign", "--verify", "--deep", "--strict", fw]);
  });

  test("fetch script rejects a checksum mismatch", () => {
    const r = fetchRun([], { serveDir: serve({ tamper: true }) });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("checksum mismatch");
    expect(r.urls).toEqual([API_URL, DOWNLOAD_URL]);
    expect(existsSync(join(r.vendor, "Sparkle.framework"))).toBe(false);
    expect(existsSync(join(r.vendor, "sparkle.version"))).toBe(false);
  });

  test("rejects a published digest that is not the pinned one", () => {
    const r = fetchRun([], {
      serveDir: serve({ digest: `sha256:${"a".repeat(64)}` }),
    });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("pinned");
    // Nothing is downloaded after the API disagrees with the pin.
    expect(r.urls).toEqual([API_URL]);
    expect(existsSync(join(r.vendor, "Sparkle.framework"))).toBe(false);
  });

  test("fails when the release API publishes no digest for the asset", () => {
    const r = fetchRun([], { serveDir: serve({ digest: null }) });
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain(`no SHA-256 digest for ${ASSET}`);
    expect(r.urls).toEqual([API_URL]);
  });

  test("keeps an existing fetch unless --force", () => {
    const first = fetchRun();
    expect(first.exitCode).toBe(0);
    const again = fetchRun([], { vendor: first.vendor });
    expect(again.exitCode).toBe(0);
    expect(again.output).toContain("already fetched");
    expect(again.urls).toEqual([]);
    const forced = fetchRun(["--force"], { vendor: first.vendor });
    expect(forced.exitCode).toBe(0);
    expect(forced.urls).toEqual([API_URL, DOWNLOAD_URL]);
  });

  test("a GitHub token goes to curl in a header file, never in argv", () => {
    const token = "ghs_not-a-real-token-1234";
    const r = fetchRun([], { env: { GITHUB_TOKEN: token } });
    expect(r.exitCode).toBe(0);
    expect(r.argv.length).toBe(2);
    for (const line of r.argv) expect(line).not.toContain(token);
    expect(r.argv[0]).toContain("-H @");
  });

  test("pins the digest GitHub publishes for Sparkle 2.10.0", () => {
    const text = readFileSync(fetchSparkle, "utf8");
    expect(text).toContain(`SPARKLE_VERSION=${VERSION}`);
    expect(text).toContain(REAL_DIGEST);
  });

  test("usage errors exit 2", () => {
    const r = fetchRun(["--bogus"]);
    expect(r.exitCode).toBe(2);
    expect(r.urls).toEqual([]);
  });

  test("vendor/ is gitignored", () => {
    expect(readFileSync(join(repoRoot, ".gitignore"), "utf8")).toContain(
      "/vendor/",
    );
  });
});

// A fake generate_appcast: logs argv and stdin (the private key), lists the
// archives folder, and writes the appcast named by -o with
// $FAKE_APPCAST_VERSION as sparkle:version and, with --embed-release-notes
// (unless FAKE_APPCAST_NO_EMBED), the archives' .md notes as its description.
const fakeGenerateAppcast = `#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >"$FAKE_APPCAST_LOG.argv"
cat >"$FAKE_APPCAST_LOG.stdin"
out=""
archives=""
embed=0
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    --ed-key-file|--download-url-prefix) shift 2 ;;
    --embed-release-notes) embed=1; shift ;;
    *) archives="$1"; shift ;;
  esac
done
ls -A "$archives" >"$FAKE_APPCAST_LOG.archives"
description=""
if [ "$embed" = 1 ] && [ -z "\${FAKE_APPCAST_NO_EMBED-}" ]; then
  description="<description sparkle:format=\\"markdown\\"><![CDATA[$(cat "$archives"/*.md)]]></description>"
fi
cat >"$out" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel><item><sparkle:version>$FAKE_APPCAST_VERSION</sparkle:version>$description</item></channel>
</rss>
EOF
`;

describe("scripts/make-appcast.sh", () => {
  const KEY = "cHJpdmF0ZS1rZXktZm9yLXRlc3RzLW9ubHktbm90LXJlYWw=";
  const PREFIX = "https://github.com/owner/polygloss/releases/download/v0.1.0/";
  let tools = "";

  beforeAll(() => {
    tools = scratch("sparkle-bin");
    writeExecutable(join(tools, "generate_appcast"), fakeGenerateAppcast);
  });

  /** dist/ as package-release.sh leaves it. */
  function makeDist(
    opts: { feed?: boolean; dmg?: boolean; version?: string } = {},
  ): string {
    const dist = scratch("dist");
    const contents = join(dist, "Polygloss.app", "Contents");
    mkdirSync(contents, { recursive: true });
    const plist: Record<string, string> = {
      CFBundleIdentifier: "dev.dak.polygloss",
      CFBundleVersion: opts.version ?? "0.1.0",
    };
    if (opts.feed !== false) {
      plist.SUFeedURL = "https://example.invalid/appcast.xml";
      plist.SUPublicEDKey = "cHVibGljLWtleQ==";
    }
    writeFileSync(join(contents, "Info.json"), JSON.stringify(plist));
    must([
      "plutil",
      "-convert",
      "xml1",
      "-o",
      join(contents, "Info.plist"),
      join(contents, "Info.json"),
    ]);
    if (opts.dmg !== false)
      writeFileSync(join(dist, "Polygloss_0.1.0_aarch64.dmg"), "disk image");
    return dist;
  }

  function appcastRun(
    dist: string | undefined,
    env: Env = {},
  ): { exitCode: number; output: string; log: string } {
    const log = join(scratch("appcast-log"), "gen");
    const r = run([makeAppcast, ...(dist === undefined ? [] : [dist])], {
      SPARKLE_BIN_DIR: tools,
      FAKE_APPCAST_LOG: log,
      FAKE_APPCAST_VERSION: "0.1.0",
      ...env,
    });
    return { ...r, log };
  }

  const withKey: Env = {
    SPARKLE_PRIVATE_ED_KEY: KEY,
    SPARKLE_DOWNLOAD_URL_PREFIX: PREFIX,
  };

  test("appcast script skips without key", () => {
    const dist = makeDist();
    const r = appcastRun(dist, { SPARKLE_DOWNLOAD_URL_PREFIX: PREFIX });
    expect(r.exitCode).toBe(0);
    expect(r.output).toContain(
      "make-appcast: appcast skipped: no SPARKLE_PRIVATE_ED_KEY",
    );
    expect(existsSync(join(dist, "appcast.xml"))).toBe(false);
    expect(existsSync(`${r.log}.argv`)).toBe(false);
    // An empty secret (GitHub passes "" for a missing one) is no key.
    const empty = appcastRun(dist, { ...withKey, SPARKLE_PRIVATE_ED_KEY: "" });
    expect(empty.exitCode).toBe(0);
    expect(empty.output).toContain("appcast skipped");
  });

  test("makes dist/appcast.xml from the dmg, the key on stdin", () => {
    const dist = makeDist();
    const r = appcastRun(dist, withKey);
    expect(r.output).toContain("make-appcast: wrote");
    expect(r.exitCode).toBe(0);
    const argv = readFileSync(`${r.log}.argv`, "utf8").trim();
    expect(argv).toMatch(
      new RegExp(
        `^--ed-key-file - --download-url-prefix ${PREFIX} -o \\S+/appcast\\.xml \\S+$`,
      ),
    );
    expect(argv).not.toContain(KEY);
    expect(readFileSync(`${r.log}.stdin`, "utf8")).toBe(KEY);
    // Only the DMG is offered to generate_appcast, never the .app.
    expect(readFileSync(`${r.log}.archives`, "utf8").trim()).toBe(
      "Polygloss_0.1.0_aarch64.dmg",
    );
    expect(readFileSync(join(dist, "appcast.xml"), "utf8")).toContain(
      "<sparkle:version>0.1.0</sparkle:version>",
    );
  });

  test("embeds SPARKLE_RELEASE_NOTES, named after the DMG, in the appcast", () => {
    const dist = makeDist();
    const notes = join(scratch("notes"), "release-notes-sparkle.md");
    writeFileSync(notes, "### Fixes\n\n- **cli:** exit 2\n");
    const r = appcastRun(dist, { ...withKey, SPARKLE_RELEASE_NOTES: notes });
    expect(r.output).toContain("make-appcast: wrote");
    expect(r.exitCode).toBe(0);
    expect(readFileSync(`${r.log}.argv`, "utf8")).toContain(
      " --embed-release-notes -o ",
    );
    // generate_appcast takes notes from the file named like the archive.
    expect(
      readFileSync(`${r.log}.archives`, "utf8").trim().split("\n"),
    ).toEqual(["Polygloss_0.1.0_aarch64.dmg", "Polygloss_0.1.0_aarch64.md"]);
    expect(readFileSync(join(dist, "appcast.xml"), "utf8")).toContain(
      '<description sparkle:format="markdown"><![CDATA[### Fixes\n\n- **cli:** exit 2]]></description>',
    );

    // Notes that do not exist, or that generate_appcast left out, fail it.
    const missing = appcastRun(makeDist(), {
      ...withKey,
      SPARKLE_RELEASE_NOTES: join(sandbox.home, "no-notes.md"),
    });
    expect(missing.exitCode).toBe(1);
    expect(missing.output).toContain(
      "no release notes at SPARKLE_RELEASE_NOTES=",
    );
    const dropped = makeDist();
    const notEmbedded = appcastRun(dropped, {
      ...withKey,
      SPARKLE_RELEASE_NOTES: notes,
      FAKE_APPCAST_NO_EMBED: "1",
    });
    expect(notEmbedded.exitCode).toBe(1);
    expect(notEmbedded.output).toContain("did not embed the release notes");
    expect(existsSync(join(dropped, "appcast.xml"))).toBe(false);
    // Without notes nothing is embedded or asked for.
    const plain = appcastRun(makeDist(), withKey);
    expect(readFileSync(`${plain.log}.argv`, "utf8")).not.toContain(
      "--embed-release-notes",
    );
  });

  test("adds the trailing slash to the download prefix", () => {
    const dist = makeDist();
    const r = appcastRun(dist, {
      ...withKey,
      SPARKLE_DOWNLOAD_URL_PREFIX: PREFIX.slice(0, -1),
    });
    expect(r.exitCode).toBe(0);
    expect(readFileSync(`${r.log}.argv`, "utf8")).toContain(
      `--download-url-prefix ${PREFIX} `,
    );
  });

  test("skips a bundle built without an appcast", () => {
    const dist = makeDist({ feed: false });
    const r = appcastRun(dist, withKey);
    expect(r.exitCode).toBe(0);
    expect(r.output).toContain(
      "appcast skipped: Polygloss.app has no SUFeedURL",
    );
    expect(existsSync(join(dist, "appcast.xml"))).toBe(false);
  });

  test("fails when sparkle:version is not the bundle's CFBundleVersion", () => {
    const dist = makeDist({ version: "0.2.0" });
    const r = appcastRun(dist, withKey);
    expect(r.exitCode).toBe(1);
    expect(r.output).toContain("sparkle:version");
    expect(existsSync(join(dist, "appcast.xml"))).toBe(false);
  });

  test("configuration errors", () => {
    const dist = makeDist();
    const cases: [Env, string][] = [
      [{ SPARKLE_DOWNLOAD_URL_PREFIX: "" }, "SPARKLE_DOWNLOAD_URL_PREFIX"],
      [{ SPARKLE_DOWNLOAD_URL_PREFIX: "http://example.invalid/" }, "https"],
      [{ SPARKLE_BIN_DIR: scratch("no-tools") }, "scripts/fetch-sparkle.sh"],
    ];
    for (const [env, message] of cases) {
      const r = appcastRun(dist, { ...withKey, ...env });
      expect({ env, exitCode: r.exitCode }).toEqual({ env, exitCode: 1 });
      expect(r.output).toContain(message);
    }
    const noDmg = appcastRun(makeDist({ dmg: false }), withKey);
    expect(noDmg.exitCode).toBe(1);
    expect(noDmg.output).toContain("no Polygloss_*_aarch64.dmg");
  });

  test("usage errors exit 2", () => {
    expect(appcastRun(undefined, withKey).exitCode).toBe(2);
    expect(appcastRun(join(sandbox.home, "none"), withKey).exitCode).toBe(2);
  });
});

type Step = {
  name?: string;
  if?: string;
  run?: string;
  env?: Record<string, string>;
};

describe("release workflow (Sparkle)", () => {
  const text = readFileSync(releaseWorkflow, "utf8");
  const steps: Step[] = (Bun.YAML.parse(text) as any).jobs.release.steps;
  const running = (needle: string) => {
    const step = steps.find((s) => s.run?.includes(needle));
    if (!step) throw new Error(`no step runs ${needle}`);
    return step;
  };

  test("fetches Sparkle before packaging, with the job's token for the API", () => {
    const fetch = running("scripts/fetch-sparkle.sh");
    expect(fetch.run?.trim()).toBe("scripts/fetch-sparkle.sh");
    expect(fetch.env?.GITHUB_TOKEN).toBe("${{ github.token }}");
    const pkg = running("scripts/package-release.sh");
    expect(steps.indexOf(fetch)).toBeLessThan(steps.indexOf(pkg));
    // The appcast URL and public key are repository variables, not secrets.
    expect(pkg.env?.POLYGLOSS_APPCAST_URL).toBe(
      "${{ vars.POLYGLOSS_APPCAST_URL }}",
    );
    expect(pkg.env?.SPARKLE_PUBLIC_ED_KEY).toBe(
      "${{ vars.SPARKLE_PUBLIC_ED_KEY }}",
    );
  });

  test("makes the appcast with the release's download prefix and publishes it", () => {
    const appcast = running("scripts/make-appcast.sh dist");
    expect(appcast.if).toBe("${{ !inputs.dry_run }}");
    expect(appcast.run).toContain(
      'SPARKLE_DOWNLOAD_URL_PREFIX="https://github.com/$GITHUB_REPOSITORY/releases/download/v$POLYGLOSS_VERSION/"',
    );
    // A release without its appcast fails before anything is published.
    expect(appcast.run).toContain("[ -f dist/appcast.xml ]");
    // Sparkle's dialog shows the release notes, without <details>.
    expect(appcast.run).toContain(
      "export SPARKLE_RELEASE_NOTES=dist/release-notes-sparkle.md",
    );
    expect(running("gh release create").run).toContain("dist/appcast.xml");
  });

  test("no step waits for T5.3 any more", () => {
    expect(text).not.toContain("arrives with T5.3");
    expect(readdirSync(join(repoRoot, "scripts"))).toEqual(
      expect.arrayContaining(["fetch-sparkle.sh", "make-appcast.sh"]),
    );
  });
});
