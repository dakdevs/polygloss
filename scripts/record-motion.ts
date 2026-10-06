// The motion recorder (plan T7.3): a review aid, not CI. It runs a motion's
// filmstrip test, `e2e_motion_<name>`, with POLYGLOSS_MOTION_DUMP, so the
// strip of one policy also writes a capture every 16.667 ms of executor time
// (frame-0000.png …, tests/support/filmstrip.rs), encodes <name>.mp4 at
// 60 fps when ffmpeg is on PATH, and prints the ui-recording-timeline
// commands that scrub it frame by frame.
//
//   bun scripts/record-motion.ts <name> [--policy full|reduced] [--out <dir>]
//
// <dir> defaults to <CARGO_TARGET_DIR or target>/motion/<name>-<policy>; its
// old frames are removed first. The test runs through scripts/cargo.sh
// (POLYGLOSS_CARGO_SH replaces it: a test seam). Exit 0 once frames were
// written (a failing test, such as a baseline mismatch, is reported: its
// frames are still the recording), 1 when none were, 2 for usage errors.
import { existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

const repoRoot = resolve(import.meta.dir, "..");

export const policies = ["full", "reduced"] as const;

/** Where ui-recording-timeline's CLI lives (the skill's own directory). */
const timelineScript =
  "$HOME/.agents/skills/ui-recording-timeline/scripts/timeline.py";

/** `value` quoted for a POSIX shell. */
function sh(value: string): string {
  return `'${value.replaceAll("'", `'\\''`)}'`;
}

/** The nextest run of the motion's filmstrip test alone. */
export function testCommand(cargo: string, name: string): string[] {
  return [
    cargo,
    "nextest",
    "run",
    "-p",
    "polygloss-app",
    "--features",
    "e2e",
    "-E",
    `binary(e2e) & test(=e2e_motion_${name})`,
  ];
}

/** The recording's frames among `files`, in order. */
export function frameNames(files: string[]): string[] {
  return files.filter((f) => /^frame-\d{4}\.png$/.test(f)).sort();
}

/** Encodes the frames at 60 fps (even dimensions, as H.264 needs). */
export function ffmpegCommand(dir: string, name: string): string[] {
  return [
    "ffmpeg",
    "-y",
    "-loglevel",
    "error",
    "-framerate",
    "60",
    "-i",
    join(dir, "frame-%04d.png"),
    "-vf",
    "pad=ceil(iw/2)*2:ceil(ih/2)*2",
    "-c:v",
    "libx264",
    "-pix_fmt",
    "yuv420p",
    join(dir, `${name}.mp4`),
  ];
}

/**
 * ui-recording-timeline's steps for the mp4: extract, candidates, and a
 * contact sheet of the filmstrip's frames (the commit, the next three, ½ and
 * the last).
 */
export function timelineCommands(
  dir: string,
  name: string,
  frames: number,
): string[] {
  const wd = join(dir, "timeline");
  const last = frames - 1;
  const sheet = [...new Set([0, 1, 2, 3, Math.floor(last / 2), last])]
    .filter((k) => k <= last)
    .sort((a, b) => a - b)
    .join(",");
  return [
    `T(){ uv run "${timelineScript}" "$@"; }`,
    `T extract ${sh(join(dir, `${name}.mp4`))} ${sh(wd)}`,
    `T candidates ${sh(wd)}`,
    `T sheet ${sh(wd)} --frames ${sheet} --name ${name}`,
  ];
}

class UsageError extends Error {}

const usage =
  "bun scripts/record-motion.ts <name> [--policy full|reduced] [--out <dir>]";

function main(argv: string[]): number {
  const { values, positionals } = parseArgs({
    args: argv,
    options: {
      policy: { type: "string", default: "full" },
      out: { type: "string" },
    },
    allowPositionals: true,
    strict: true,
  });
  const [name, ...rest] = positionals;
  if (!name || rest.length > 0 || !/^[a-z0-9_-]+$/.test(name))
    throw new UsageError("give one motion name (e2e_motion_<name>)");
  const policy = policies.find((p) => p === values.policy);
  if (!policy)
    throw new UsageError(`unknown policy ${JSON.stringify(values.policy)}`);
  const targetDir = resolve(repoRoot, process.env.CARGO_TARGET_DIR || "target");
  const dir = resolve(
    values.out ?? join(targetDir, "motion", `${name}-${policy}`),
  );
  if (existsSync(dir))
    for (const old of frameNames(readdirSync(dir)))
      rmSync(join(dir, old), { force: true });
  mkdirSync(dir, { recursive: true });

  const cargo =
    process.env.POLYGLOSS_CARGO_SH || join(repoRoot, "scripts", "cargo.sh");
  const run = Bun.spawnSync(testCommand(cargo, name), {
    cwd: repoRoot,
    env: {
      ...process.env,
      POLYGLOSS_MOTION_DUMP: dir,
      POLYGLOSS_MOTION_DUMP_POLICY: policy,
    },
    stdout: "inherit",
    stderr: "inherit",
  });
  if (run.exitCode !== 0)
    process.stderr.write(
      `record-motion: e2e_motion_${name} exited with ${run.exitCode}; keeping the frames it wrote\n`,
    );
  const frames = frameNames(readdirSync(dir));
  if (frames.length === 0) {
    process.stderr.write(
      `record-motion: e2e_motion_${name} wrote no frames to ${dir} (is there such a test, with a ${policy} filmstrip?)\n`,
    );
    return 1;
  }
  process.stdout.write(
    `${frames.length} frames in ${dir}: ${frames[0]} … ${frames.at(-1)} (one per 16.667 ms from the commit frame)\n`,
  );
  if (Bun.which("ffmpeg", { PATH: process.env.PATH ?? "" })) {
    const encode = Bun.spawnSync(ffmpegCommand(dir, name), {
      stdout: "inherit",
      stderr: "inherit",
    });
    if (encode.exitCode === 0)
      process.stdout.write(`${join(dir, `${name}.mp4`)} (60 fps)\n`);
    else
      process.stderr.write(
        `record-motion: ffmpeg exited with ${encode.exitCode}; no ${name}.mp4\n`,
      );
  } else
    process.stdout.write(
      `ffmpeg is not on PATH: no ${name}.mp4 (the frames are the recording)\n`,
    );
  process.stdout.write(
    `Scrub it with ui-recording-timeline:\n${timelineCommands(
      dir,
      name,
      frames.length,
    )
      .map((l) => `  ${l}`)
      .join("\n")}\n`,
  );
  return 0;
}

if (import.meta.main) {
  try {
    process.exit(main(process.argv.slice(2)));
  } catch (e) {
    const usageError =
      e instanceof UsageError ||
      (e instanceof TypeError &&
        "code" in e &&
        String(e.code).startsWith("ERR_PARSE_ARGS"));
    if (!usageError) throw e;
    process.stderr.write(`record-motion: ${e.message}\nusage: ${usage}\n`);
    process.exit(2);
  }
}
