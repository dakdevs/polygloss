import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");

// Names that keep their conventional casing (README, licenses, Cargo, insta,
// Claude Code skills, Apple bundles). Everything else is lowercase kebab-case.
const allowedNames = new Set([
  "README.md",
  "AGENTS.md",
  "CONTRIBUTING.md",
  "NOTICE",
  "Cargo.toml",
  "Cargo.lock",
  "SKILL.md",
  "Info.plist",
]);
const licenseName = /^LICENSE-[A-Z0-9.-]+$/;
// Dot-separated parts, each lowercase kebab-case: `cargo-sh.test.ts`, `bun.lock`.
const kebabName = /^[a-z0-9]+(?:-[a-z0-9]+)*(?:\.[a-z0-9]+(?:-[a-z0-9]+)*)*$/;
// Rust module files and module directories (ADR-0018).
const rustModuleFile = /^[a-z][a-z0-9_]*\.rs$/;
const rustModuleDir = /^[a-z][a-z0-9_]*$/;
// Matches `interface Foo`, `export interface`, `export default interface`, `declare interface`.
const interfaceDeclaration =
  /^\s*(?:export\s+(?:default\s+)?)?(?:declare\s+)?interface\s/m;
const typeScriptFile = /\.(?:ts|tsx|mts|cts)$/;

// Returns why `path` (repo-relative, `/`-separated) breaks the naming rules, or null.
function namingViolation(path: string): string | null {
  const segments = path.split("/");
  const name = segments[segments.length - 1] ?? "";
  // Rust module directories (snake_case) exist only inside a crate, next to .rs files
  // or the insta snapshots those modules own.
  const isRustOwned =
    segments[0] === "crates" &&
    (name.endsWith(".rs") || name.endsWith(".snap"));
  for (const [i, dir] of segments.slice(0, -1).entries()) {
    if (dir.startsWith(".") || kebabName.test(dir)) continue;
    // An Icon Composer document keeps Apple's folder name: `<name>.icon/Assets/`.
    if (dir === "Assets" && segments[i - 1]?.endsWith(".icon")) continue;
    if (isRustOwned && i >= 2 && rustModuleDir.test(dir)) continue;
    return `directory "${dir}" is not kebab-case`;
  }
  if (allowedNames.has(name) || licenseName.test(name)) return null;
  if (name.startsWith(".")) return null;
  if (name.endsWith(".snap")) return null;
  if (name.endsWith(".rs")) {
    return rustModuleFile.test(name)
      ? null
      : `Rust file "${name}" is not snake_case`;
  }
  return kebabName.test(name) ? null : `file "${name}" is not kebab-case`;
}

let sandbox = "";
let gitEnv: Record<string, string> = {};

beforeAll(() => {
  sandbox = realpathSync(mkdtempSync(join(tmpdir(), "polygloss-hygiene-")));
  writeFileSync(join(sandbox, "gitconfig"), "");
  gitEnv = {
    HOME: sandbox,
    XDG_CONFIG_HOME: join(sandbox, ".config"),
    GIT_CONFIG_GLOBAL: join(sandbox, "gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
    PATH: process.env.PATH ?? "/usr/bin:/bin",
  };
});

afterAll(() => {
  if (sandbox) rmSync(sandbox, { recursive: true, force: true });
});

// Tracked files that still exist in the working tree.
function trackedFiles(): string[] {
  const r = Bun.spawnSync(["git", "ls-files", "-z"], {
    cwd: repoRoot,
    env: gitEnv,
  });
  if (r.exitCode !== 0) {
    throw new Error(`git ls-files failed: ${r.stderr.toString()}`);
  }
  return r.stdout
    .toString()
    .split("\0")
    .filter((p) => p !== "" && existsSync(join(repoRoot, p)));
}

describe("naming rules", () => {
  test("accept kebab-case and the documented exceptions", () => {
    for (const path of [
      "docs/adr/0001-native-gpui-only.md",
      "tests/scripts/cargo-sh.test.ts",
      ".github/workflows/ci.yml",
      ".config/nextest.toml",
      ".gitignore",
      "bun.lock",
      "README.md",
      "docs/adr/README.md",
      "LICENSE-MIT",
      "LICENSE-APACHE",
      "NOTICE",
      "crates/polygloss-core/Cargo.toml",
      "Cargo.lock",
      "crates/polygloss-core/src/lib.rs",
      "crates/polygloss-core/src/review_domain/draft_state.rs",
      "crates/polygloss-core/src/review_domain/snapshots/polygloss_core__ids__golden_vectors.snap",
      "crates/polygloss-app/tests/e2e/main.rs",
      "plugins/polygloss/skills/review/SKILL.md",
      "packaging/Info.plist",
      "packaging/polygloss.icon/Assets/1-deleted.svg",
      "assets/fonts/lilex/lilex-regular.ttf",
      "assets/themes/pierre-light.json",
    ]) {
      expect({ path, violation: namingViolation(path) }).toEqual({
        path,
        violation: null,
      });
    }
  });

  test("reject snake_case, PascalCase, spaces and unlisted uppercase names", () => {
    for (const path of [
      "scripts/make_fixture_repo.ts",
      "assets/fonts/lilex/Lilex-Regular.ttf",
      "docs/Design.md",
      "CHANGELOG.md",
      "crates/polygloss_core/Cargo.toml",
      "crates/polygloss-core/src/Store.rs",
      "crates/polygloss-core/src/draft-state.rs",
      "crates/polygloss-core/fixtures/some_dir/input.json",
      "tests/support_files/sandbox.ts",
      "assets/icons/app icon.png",
      "packaging/Assets/1-deleted.svg",
      "packaging/AppIcon.icon/Assets/1-deleted.svg",
      "fixtures/foo--bar.txt",
    ]) {
      expect({ path, rejected: namingViolation(path) !== null }).toEqual({
        path,
        rejected: true,
      });
    }
  });

  test("interface pattern flags declarations only", () => {
    for (const src of [
      "interface Foo {}",
      "export interface Foo {}",
      "  export default interface Foo {}",
      "declare interface Foo {}",
      "const a = 1;\nexport declare interface Foo {}",
    ]) {
      expect({ src, flagged: interfaceDeclaration.test(src) }).toEqual({
        src,
        flagged: true,
      });
    }
    for (const src of [
      "type Foo = { a: string };",
      "// an interface in a comment",
      "const interfaceName = 'x';",
      "function f(opts: { interface: string }) {}",
    ]) {
      expect({ src, flagged: interfaceDeclaration.test(src) }).toEqual({
        src,
        flagged: false,
      });
    }
  });
});

describe("repo hygiene", () => {
  test("every tracked file name is kebab-case", () => {
    const files = trackedFiles();
    expect(files.length).toBeGreaterThan(0);
    const violations = files
      .map((path) => ({ path, violation: namingViolation(path) }))
      .filter((v) => v.violation !== null);
    expect(violations).toEqual([]);
  });

  test("no TypeScript file declares an interface", () => {
    const tsFiles = trackedFiles().filter((p) => typeScriptFile.test(p));
    expect(tsFiles.length).toBeGreaterThan(0);
    const offenders = tsFiles.filter((p) =>
      interfaceDeclaration.test(readFileSync(join(repoRoot, p), "utf8")),
    );
    expect(offenders).toEqual([]);
  });
});
