// App icons (plan M6 "Icons", T6.3): an icon no asset source registers draws
// nothing, silently. Every `IconName::X` or `Lucide::X` (the app's alias of
// gpui-kit-assets' `IconName`) in the app's sources and every
// `"icons/<name>.svg"` literal in the app's and the viewport's sources must
// be one of `AppIcons` (crates/polygloss-app/src/assets.rs) or one of
// gpui-kit-assets' `default-icons.txt` (the bundle `gpui_kit::assets::Assets`
// embeds). The catalog and the defaults come from the gpui-kit-assets crate
// cargo resolves (`cargo metadata`), its file names turned into variant names
// the way its build script does.
import { afterAll, expect, setDefaultTimeout, test } from "bun:test";
import {
  cpSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

// `cargo metadata` over the workspace.
setDefaultTimeout(120_000);

const repoRoot = resolve(import.meta.dir, "../..");
const appSrc = "crates/polygloss-app/src";
const viewportSrc = "crates/polygloss-viewport/src";

const sandbox = makeSandbox();
const scratch = mkdtempSync(join(tmpdir(), "polygloss-app-icons-"));
afterAll(() => {
  sandbox.cleanup();
  rmSync(scratch, { recursive: true, force: true });
});

/** The directory of package `name` in `cargo metadata` output. */
function packageDir(metadata: unknown, name: string): string {
  if (
    typeof metadata !== "object" ||
    metadata === null ||
    !("packages" in metadata) ||
    !Array.isArray(metadata.packages)
  )
    throw new Error("cargo metadata printed no package list");
  for (const pkg of metadata.packages) {
    if (
      typeof pkg === "object" &&
      pkg !== null &&
      "name" in pkg &&
      pkg.name === name &&
      "manifest_path" in pkg &&
      typeof pkg.manifest_path === "string"
    )
      return dirname(pkg.manifest_path);
  }
  throw new Error(`cargo metadata lists no ${name}`);
}

function kitAssetsDir(): string {
  const r = Bun.spawnSync(
    [
      join(repoRoot, "scripts", "cargo.sh"),
      "metadata",
      "--format-version",
      "1",
      "--locked",
    ],
    {
      cwd: repoRoot,
      // The sandbox plus the real (read-only) toolchain and registry.
      env: {
        ...sandbox.env,
        CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), ".cargo"),
        RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"),
      },
    },
  );
  if (r.exitCode !== 0)
    throw new Error(
      `cargo metadata failed with exit code ${r.exitCode}: ${r.stderr.toString()}`,
    );
  return packageDir(JSON.parse(r.stdout.toString()), "gpui-kit-assets");
}

/** gpui-kit-assets' build script: `rotate-ccw-clock` → `RotateCcwClock`. */
function variantName(stem: string): string {
  return stem
    .split(/[-_.]/)
    .filter((part) => part.length > 0)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1).toLowerCase())
    .join("");
}

/** Every icon by variant name, and the paths of the default bundle. */
function readKit(dir: string): {
  catalog: Map<string, string>;
  defaults: Set<string>;
} {
  const catalog = new Map<string, string>();
  for (const file of readdirSync(join(dir, "assets", "icons"))) {
    if (!file.endsWith(".svg")) continue;
    const stem = file.slice(0, -".svg".length);
    catalog.set(variantName(stem), `icons/${stem}.svg`);
  }
  const defaults = new Set(
    readFileSync(join(dir, "default-icons.txt"), "utf8")
      .split("\n")
      .filter((line) => line.length > 0),
  );
  return { catalog, defaults };
}

const kit = readKit(kitAssetsDir());

/** The names in `icon_assets!(pub AppIcons, [...])` of `root`'s app. */
function appIcons(root: string): string[] {
  const source = readFileSync(join(root, appSrc, "assets.rs"), "utf8");
  const list = /icon_assets!\(\s*pub AppIcons,\s*\[([^\]]*)\]/.exec(
    source,
  )?.[1];
  if (list === undefined) throw new Error("assets.rs has no AppIcons list");
  return list
    .split(",")
    .map((name) => name.trim())
    .filter((name) => name.length > 0);
}

function rustFiles(dir: string): string[] {
  return readdirSync(dir, { recursive: true, encoding: "utf8" })
    .filter((file) => file.endsWith(".rs"))
    .map((file) => join(dir, file));
}

/**
 * The icon uses found under `root` (`IconName::X`, `"icons/x.svg"`) and why
 * any of them would draw nothing.
 */
function iconProblems(root: string): { uses: Set<string>; problems: string[] } {
  const problems: string[] = [];
  const registered = new Set(kit.defaults);
  for (const name of appIcons(root)) {
    const path = kit.catalog.get(name);
    if (path === undefined)
      problems.push(`assets.rs: AppIcons lists ${name}, no gpui-kit icon`);
    else registered.add(path);
  }
  const known = new Set(kit.catalog.values());
  const uses = new Set<string>();
  const check = (file: string, use: string, path: string | undefined) => {
    uses.add(use);
    const where = relative(root, file);
    if (path === undefined)
      problems.push(`${where}: ${use} is no gpui-kit icon`);
    else if (!registered.has(path))
      problems.push(
        `${where}: ${use} (${path}) is neither in AppIcons nor a gpui-kit default`,
      );
  };
  for (const file of rustFiles(join(root, appSrc))) {
    const text = readFileSync(file, "utf8");
    for (const [, alias, name] of text.matchAll(
      /\b(IconName|Lucide)::([A-Z][A-Za-z0-9]*)/g,
    )) {
      // `IconName::ALL`, the catalog itself.
      if (name === undefined || name === "ALL") continue;
      check(file, `${alias}::${name}`, kit.catalog.get(name));
    }
  }
  for (const dir of [appSrc, viewportSrc]) {
    for (const file of rustFiles(join(root, dir))) {
      const text = readFileSync(file, "utf8");
      for (const [, path] of text.matchAll(/"(icons\/[^"]+\.svg)"/g)) {
        if (path === undefined) continue;
        check(file, `"${path}"`, known.has(path) ? path : undefined);
      }
    }
  }
  return { uses, problems };
}

test("every icon the app and viewport use is registered", () => {
  const { uses, problems } = iconProblems(repoRoot);
  expect(problems).toEqual([]);
  // The scan sees the chrome's icons (AppIcons) and the kit's own.
  expect(uses).toContain("IconName::ListTree");
  expect(uses).toContain("IconName::RotateCcwClock");
  expect(uses).toContain("IconName::Close");
  expect(uses).toContain("Lucide::GitBranch");
});

test("an unregistered icon fails the check", () => {
  const root = join(scratch, "copy");
  for (const dir of [appSrc, viewportSrc])
    cpSync(join(repoRoot, dir), join(root, dir), { recursive: true });
  writeFileSync(
    join(root, appSrc, "planted.rs"),
    "fn f() { let _ = (IconName::Rocket, IconName::NoSuchIcon, Lucide::NoSuchLucide); }\n",
  );
  writeFileSync(
    join(root, viewportSrc, "planted.rs"),
    'const P: &str = "icons/anchor.svg";\n',
  );
  const assets = join(root, appSrc, "assets.rs");
  writeFileSync(
    assets,
    readFileSync(assets, "utf8").replace("Dot,", "Dot,\n        NotAnIcon,"),
  );
  const { problems } = iconProblems(root);
  expect(problems.sort()).toEqual(
    [
      "assets.rs: AppIcons lists NotAnIcon, no gpui-kit icon",
      `${appSrc}/planted.rs: IconName::Rocket (icons/rocket.svg) is neither in AppIcons nor a gpui-kit default`,
      `${appSrc}/planted.rs: IconName::NoSuchIcon is no gpui-kit icon`,
      `${appSrc}/planted.rs: Lucide::NoSuchLucide is no gpui-kit icon`,
      `${viewportSrc}/planted.rs: "icons/anchor.svg" (icons/anchor.svg) is neither in AppIcons nor a gpui-kit default`,
    ].sort(),
  );
});
