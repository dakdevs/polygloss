// A stand-in Sparkle.framework (plan T5.3, library-choices §14): Sparkle
// 2.10.0's layout with copies of /usr/bin/true as its Mach-O files, so
// codesign and the packaging scripts treat it like the real one without a
// download.
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

function machO(path: string): void {
  copyFileSync("/usr/bin/true", path);
  chmodSync(path, 0o755);
}

function writePlist(path: string, value: Record<string, string>): void {
  writeFileSync(`${path}.json`, JSON.stringify(value));
  const r = Bun.spawnSync([
    "plutil",
    "-convert",
    "xml1",
    "-o",
    path,
    `${path}.json`,
  ]);
  rmSync(`${path}.json`);
  if (r.exitCode !== 0)
    throw new Error(`plutil failed for ${path}: ${r.stderr.toString()}`);
}

/**
 * `<dir>/Sparkle.framework` laid out as Sparkle ships it: `Versions/B` with
 * the `Sparkle` library, `Autoupdate`, `Updater.app` and (with `xpc`) the
 * `XPCServices` Installer and Downloader, `Versions/Current -> B`, and the
 * top-level symlinks (`XPCServices` only with `xpc`). Returns its path.
 */
export function makeSparkleFramework(
  dir: string,
  opts: { xpc?: boolean } = {},
): string {
  const fw = join(dir, "Sparkle.framework");
  const b = join(fw, "Versions", "B");
  mkdirSync(join(b, "Resources"), { recursive: true });
  machO(join(b, "Sparkle"));
  machO(join(b, "Autoupdate"));
  writePlist(join(b, "Resources", "Info.plist"), {
    CFBundleExecutable: "Sparkle",
    CFBundleIdentifier: "org.sparkle-project.Sparkle",
    CFBundlePackageType: "FMWK",
    CFBundleShortVersionString: "2.10.0",
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
  const links = ["Sparkle", "Autoupdate", "Updater.app", "Resources"];
  if (opts.xpc) links.push("XPCServices");
  for (const name of links)
    symlinkSync(`Versions/Current/${name}`, join(fw, name));
  return fw;
}
