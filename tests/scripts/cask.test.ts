// The Homebrew cask template and scripts/bump-tap.ts (plan T5.4, design §21,
// ADR-0019). The rendered cask is evaluated by the system Ruby against a
// small stand-in for Homebrew's cask DSL, so the test sees the stanzas Ruby
// sees (string interpolation included) without running `brew`, which would
// read and write the real ~/Library/Caches/Homebrew.
import { afterAll, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { renderCask } from "../../scripts/bump-tap";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts/bump-tap.ts");
const template = join(repoRoot, "packaging/homebrew/polygloss.rb.tmpl");

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

const SHA = "0123456789abcdef".repeat(4);
const good = { version: "20261005.1", sha256: SHA, repo: "dakdevs/polygloss" };

/** Every stanza the cask calls, in order, as Ruby evaluated it. */
type Stanza = { name: string; args: unknown[] };

// A stand-in for Homebrew's cask DSL: records each call with its evaluated
// arguments (hashes as objects, symbols as ":name").
const DSL = String.raw`
require "json"
class Recorder
  attr_reader :calls
  def initialize; @calls = []; @version = nil; end
  def appdir; "/Applications"; end
  def version(v = nil)
    return @version if v.nil?
    @version = v
    @calls << { "name" => "version", "args" => [v] }
  end
  def plain(x)
    case x
    when Symbol then ":#{x}"
    when Hash then x.map { |k, v| [plain(k), plain(v)] }.to_h
    when Array then x.map { |v| plain(v) }
    else x
    end
  end
  def method_missing(name, *args, &block)
    @calls << { "name" => name.to_s, "args" => plain(args) }
    instance_eval(&block) if block
  end
  def respond_to_missing?(*); true; end
end
def cask(token, &block)
  r = Recorder.new
  r.instance_eval(&block)
  puts JSON.generate({ "token" => token, "calls" => r.calls })
end
load ARGV[0]
`;

function evalCask(source: string): { token: string; calls: Stanza[] } {
  const dir = join(sandbox.home, `cask-${crypto.randomUUID()}`);
  mkdirSync(dir, { recursive: true });
  const file = join(dir, "polygloss.rb");
  writeFileSync(file, source);
  const r = Bun.spawnSync(["/usr/bin/ruby", "-e", DSL, file], {
    env: sandbox.env,
  });
  if (r.exitCode !== 0)
    throw new Error(`ruby failed (${r.exitCode}): ${r.stderr.toString()}`);
  return JSON.parse(r.stdout.toString());
}

function bump(args: string[]): {
  exitCode: number;
  stdout: string;
  stderr: string;
} {
  const r = Bun.spawnSync([process.execPath, script, ...args], {
    cwd: repoRoot,
    env: sandbox.env,
  });
  return {
    exitCode: r.exitCode,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

function tapDir(): string {
  const dir = join(sandbox.home, `tap-${crypto.randomUUID()}`);
  mkdirSync(dir, { recursive: true });
  return dir;
}

describe("cask template", () => {
  test("cask renders with binary stanza and arm64 sonoma", () => {
    const { token, calls } = evalCask(renderCask(good));
    expect(token).toBe("polygloss");
    const args = (name: string) =>
      calls.filter((c) => c.name === name).map((c) => c.args);
    expect(args("version")).toEqual([["20261005.1"]]);
    expect(args("sha256")).toEqual([[SHA]]);
    expect(args("url")).toEqual([
      [
        "https://github.com/dakdevs/polygloss/releases/download/v20261005.1/Polygloss_20261005.1_aarch64.dmg",
      ],
    ]);
    expect(args("homepage")).toEqual([
      ["https://github.com/dakdevs/polygloss"],
    ]);
    expect(args("app")).toEqual([["Polygloss.app"]]);
    expect(args("binary")).toEqual([
      [
        "/Applications/Polygloss.app/Contents/MacOS/polygloss-cli",
        { ":target": "polygloss" },
      ],
    ]);
    expect(args("auto_updates")).toEqual([[true]]);
    expect(args("depends_on")).toEqual(
      expect.arrayContaining([
        [{ ":arch": ":arm64" }],
        [{ ":macos": ">= :sonoma" }],
      ]),
    );
    // Zap removes what the app writes (design §7 paths), never the repos.
    const zap = args("zap");
    expect(zap.length).toBe(1);
    const trash = (zap[0]![0] as Record<string, string[]>)[":trash"]!;
    expect(trash).toEqual(
      expect.arrayContaining([
        "~/Library/Application Support/polygloss",
        "~/Library/Caches/polygloss",
        "~/Library/Logs/polygloss",
        "~/Library/Preferences/dev.dak.polygloss.plist",
      ]),
    );
  });

  test("the template's binary stanza is literal Ruby, not rendered text", () => {
    const text = readFileSync(template, "utf8");
    expect(text).toContain(
      'binary "#{appdir}/Polygloss.app/Contents/MacOS/polygloss-cli", target: "polygloss"',
    );
    expect(text).toContain('app "Polygloss.app"');
    expect(text).toContain("auto_updates true");
    expect(text).toContain("depends_on arch: :arm64");
    expect(text).toContain('depends_on macos: ">= :sonoma"');
  });

  test("render fills every placeholder", () => {
    const out = renderCask(good);
    expect(out).not.toMatch(/\{\{|\}\}/);
    expect(out).toContain(`sha256 "${SHA}"`);
    expect(out).toContain('version "20261005.1"');
  });

  test("render refuses values that are not a version, a sha256 or owner/name", () => {
    expect(() => renderCask({ ...good, version: "" })).toThrow(/version/);
    expect(() => renderCask({ ...good, version: "v20261005.1" })).toThrow(
      /version/,
    );
    // Releases are CalVer (ADR-0019), never the crate's semver.
    expect(() => renderCask({ ...good, version: "1.2.3" })).toThrow(/version/);
    expect(() =>
      renderCask({ ...good, version: '20261005.1" ; system "x' }),
    ).toThrow(/version/);
    expect(() => renderCask({ ...good, sha256: "abc" })).toThrow(/sha256/);
    expect(() => renderCask({ ...good, sha256: SHA.toUpperCase() })).toThrow(
      /sha256/,
    );
    expect(() => renderCask({ ...good, repo: "polygloss" })).toThrow(/repo/);
    expect(() => renderCask({ ...good, repo: "a/b/c" })).toThrow(/repo/);
    expect(() => renderCask({ ...good, repo: 'a/b"#{x}' })).toThrow(/repo/);
  });

  test("a later release of the day renders", () => {
    const { calls } = evalCask(renderCask({ ...good, version: "20261005.12" }));
    const url = calls.find((c) => c.name === "url")!.args[0];
    expect(url).toBe(
      "https://github.com/dakdevs/polygloss/releases/download/v20261005.12/Polygloss_20261005.12_aarch64.dmg",
    );
  });
});

describe("bump-tap.ts", () => {
  test("bump-tap refuses missing sha", () => {
    const out = tapDir();
    const r = bump([
      "--version",
      "20261005.1",
      "--repo",
      "dakdevs/polygloss",
      "--out",
      out,
    ]);
    expect(r.exitCode).toBe(2);
    expect(r.stderr).toContain("--sha256");
    expect(existsSync(join(out, "Casks"))).toBe(false);
  });

  test("bump-tap refuses every other missing or empty value", () => {
    const out = tapDir();
    const all: Record<string, string> = {
      "--version": "20261005.1",
      "--sha256": SHA,
      "--repo": "dakdevs/polygloss",
      "--out": out,
    };
    for (const missing of Object.keys(all)) {
      for (const empty of [false, true]) {
        const args = Object.entries(all).flatMap(([k, v]) =>
          k === missing ? (empty ? [k, ""] : []) : [k, v],
        );
        const r = bump(args);
        expect({ missing, empty, code: r.exitCode }).toEqual({
          missing,
          empty,
          code: 2,
        });
        expect(r.stderr).toContain(missing);
      }
    }
    expect(existsSync(join(out, "Casks"))).toBe(false);
  });

  test("bump-tap refuses a malformed sha and a missing tap checkout", () => {
    const bad = bump([
      "--version",
      "20261005.1",
      "--sha256",
      "not-a-sha",
      "--repo",
      "dakdevs/polygloss",
      "--out",
      tapDir(),
    ]);
    expect(bad.exitCode).toBe(2);
    expect(bad.stderr).toContain("sha256");
    const gone = bump([
      "--version",
      "20261005.1",
      "--sha256",
      SHA,
      "--repo",
      "dakdevs/polygloss",
      "--out",
      join(sandbox.home, "no-such-tap"),
    ]);
    expect(gone.exitCode).toBe(2);
    expect(gone.stderr).toContain("no-such-tap");
  });

  test("bump-tap writes Casks/polygloss.rb into the tap checkout", () => {
    const out = tapDir();
    // An older cask is replaced.
    mkdirSync(join(out, "Casks"));
    writeFileSync(join(out, "Casks/polygloss.rb"), "old\n");
    const r = bump([
      "--version",
      "20261005.1",
      "--sha256",
      SHA,
      "--repo",
      "dakdevs/polygloss",
      "--out",
      out,
    ]);
    expect(r.stderr).toBe("");
    expect(r.exitCode).toBe(0);
    const written = readFileSync(join(out, "Casks/polygloss.rb"), "utf8");
    expect(written).toBe(renderCask(good));
    expect(r.stdout).toContain("Casks/polygloss.rb");
  });

  test("usage error on unknown flags", () => {
    const r = bump(["--version", "20261005.1", "--nope"]);
    expect(r.exitCode).toBe(2);
    expect(r.stderr).toMatch(/usage/i);
  });
});

describe("release.yml tap bump", () => {
  const release = readFileSync(
    join(repoRoot, ".github/workflows/release.yml"),
    "utf8",
  );

  test("calls bump-tap.ts without the pre-T5.4 guard", () => {
    expect(release).not.toContain("arrives with T5.4");
    expect(release).toContain(
      'bun scripts/bump-tap.ts --version "$version" --sha256 "$sha" --repo "$GITHUB_REPOSITORY" --out "$tap"',
    );
  });

  test("the cask downloads the DMG name the workflow uploads", () => {
    expect(release).toContain(
      '"dist/Polygloss_${POLYGLOSS_VERSION}_aarch64.dmg"',
    );
    expect(renderCask(good)).toContain("Polygloss_#{version}_aarch64.dmg");
  });
});
