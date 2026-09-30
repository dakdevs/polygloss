#!/usr/bin/env bun
// Renders the Homebrew cask (packaging/homebrew/polygloss.rb.tmpl, plan T5.4)
// for one release into a checkout of the tap, as Casks/polygloss.rb.
// release.yml runs it on `v*` tags, then commits and pushes the tap; it never
// touches git or the network itself.
//
//   bun scripts/bump-tap.ts --version <v> --sha256 <sha> --repo <owner/name> --out <tap checkout>
//
// --version is the release version without the `v` (the tag is `v<version>`),
// --sha256 the DMG's SHA-256 (lowercase hex), --repo the app's GitHub repo
// (the DMG's GitHub Releases home). Any missing or malformed value is refused
// with exit 2 before anything is written.
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const templatePath = resolve(
  import.meta.dir,
  "../packaging/homebrew/polygloss.rb.tmpl",
);

const USAGE =
  "usage: bun scripts/bump-tap.ts --version <v> --sha256 <sha> --repo <owner/name> --out <tap checkout>";

// Every value lands inside a Ruby string literal, so each pattern also keeps
// out quotes, `#{…}` and backslashes.
const VERSION = /^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;
const SHA256 = /^[0-9a-f]{64}$/;
const REPO = /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?\/[A-Za-z0-9._-]+$/;

/** The cask for one release. Throws when a value is malformed. */
export function renderCask({
  version,
  sha256,
  repo,
}: {
  version: string;
  sha256: string;
  repo: string;
}): string {
  if (!VERSION.test(version))
    throw new Error(
      `version must look like 1.2.3 or 1.2.3-beta.1 (no v): ${JSON.stringify(version)}`,
    );
  if (!SHA256.test(sha256))
    throw new Error(
      `sha256 must be 64 lowercase hex digits: ${JSON.stringify(sha256)}`,
    );
  if (!REPO.test(repo))
    throw new Error(
      `repo must be a GitHub owner/name: ${JSON.stringify(repo)}`,
    );
  const values: Record<string, string> = { version, sha256, repo };
  const out = readFileSync(templatePath, "utf8").replace(
    /\{\{(\w+)\}\}/g,
    (whole, key: string) => {
      const value = values[key];
      if (value === undefined)
        throw new Error(`unknown placeholder ${whole} in ${templatePath}`);
      return value;
    },
  );
  return out;
}

function fail(message: string): never {
  console.error(`bump-tap: ${message}`);
  process.exit(2);
}

function main(argv: string[]): void {
  let values: Record<string, string | boolean | undefined>;
  try {
    ({ values } = parseArgs({
      args: argv,
      options: {
        version: { type: "string" },
        sha256: { type: "string" },
        repo: { type: "string" },
        out: { type: "string" },
      },
      strict: true,
      allowPositionals: false,
    }));
  } catch (e) {
    fail(`${(e as Error).message}\n${USAGE}`);
  }
  const get = (key: string): string => {
    const value = values[key];
    if (typeof value !== "string" || value.trim() === "")
      fail(`missing --${key}\n${USAGE}`);
    return value;
  };
  const version = get("version");
  const sha256 = get("sha256");
  const repo = get("repo");
  const out = get("out");

  let cask: string;
  try {
    cask = renderCask({ version, sha256, repo });
  } catch (e) {
    fail((e as Error).message);
  }
  let isDir = false;
  try {
    isDir = statSync(out).isDirectory();
  } catch {
    // Reported below.
  }
  if (!isDir) fail(`--out ${out} is not a directory (clone the tap first)`);

  const dir = join(out, "Casks");
  mkdirSync(dir, { recursive: true });
  const file = join(dir, "polygloss.rb");
  writeFileSync(file, cask);
  console.log(`bump-tap: wrote ${file} (polygloss ${version})`);
}

if (import.meta.main) main(process.argv.slice(2));
