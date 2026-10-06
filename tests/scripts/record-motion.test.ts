// The motion recorder (plan T7.3): it runs a motion's filmstrip test with
// POLYGLOSS_MOTION_DUMP, names the frames it wrote, encodes an mp4 when
// ffmpeg is on PATH and prints the ui-recording-timeline commands. Driven
// here by a fake cargo.sh (POLYGLOSS_CARGO_SH) and a fake ffmpeg, so no test
// binary is built.
import { afterAll, describe, expect, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

// The fake test run: it logs its arguments and the recording's environment,
// then writes FAKE_FRAMES (default 3) frames into the dump directory.
const fakeCargo = join(sandbox.home, "fake-cargo.sh");
const cargoLog = join(sandbox.home, "fake-cargo.log");
writeFileSync(
  fakeCargo,
  `#!/bin/sh
printf '%s\\n' "$*" >> "${cargoLog}"
printf 'dump=%s policy=%s\\n' "$POLYGLOSS_MOTION_DUMP" "$POLYGLOSS_MOTION_DUMP_POLICY" >> "${cargoLog}"
k=0
while [ "$k" -lt "\${FAKE_FRAMES:-3}" ]; do
  printf 'png' > "$POLYGLOSS_MOTION_DUMP/$(printf 'frame-%04d.png' "$k")"
  k=$((k + 1))
done
exit \${FAKE_EXIT:-0}
`,
);
chmodSync(fakeCargo, 0o755);

// A directory holding only a fake ffmpeg, which logs its arguments and
// writes its last one (the output file).
const ffmpegBin = join(sandbox.home, "ffmpeg-bin");
const ffmpegLog = join(sandbox.home, "fake-ffmpeg.log");
mkdirSync(ffmpegBin);
writeFileSync(
  join(ffmpegBin, "ffmpeg"),
  `#!/bin/sh
printf '%s\\n' "$*" >> "${ffmpegLog}"
for last; do :; done
printf 'mp4' > "$last"
`,
);
chmodSync(join(ffmpegBin, "ffmpeg"), 0o755);

// The system's own tools, without Homebrew's (where a real ffmpeg lives).
const systemPath = "/usr/bin:/bin";

function record(
  args: string[],
  env: Record<string, string> = {},
): { code: number | null; stdout: string; stderr: string } {
  const r = Bun.spawnSync(
    [process.execPath, "scripts/record-motion.ts", ...args],
    {
      cwd: repoRoot,
      env: {
        ...sandbox.env,
        PATH: systemPath,
        POLYGLOSS_CARGO_SH: fakeCargo,
        ...env,
      },
    },
  );
  return {
    code: r.exitCode,
    stdout: r.stdout.toString(),
    stderr: r.stderr.toString(),
  };
}

describe("record-motion", () => {
  test("names_frames_and_skips_the_mp4_without_ffmpeg", () => {
    writeFileSync(cargoLog, "");
    const out = join(sandbox.home, "probe-reduced");
    const r = record(["probe", "--policy", "reduced", "--out", out]);
    expect({ code: r.code, stderr: r.stderr }).toMatchObject({ code: 0 });
    // One filmstrip test, the Reduced strip recording into `out`.
    expect(readFileSync(cargoLog, "utf8").trim().split("\n")).toEqual([
      "nextest run -p polygloss-app --features e2e -E binary(e2e) & test(=e2e_motion_probe)",
      `dump=${out} policy=reduced`,
    ]);
    expect(r.stdout).toContain(
      `3 frames in ${out}: frame-0000.png … frame-0002.png`,
    );
    expect(existsSync(join(out, "probe.mp4"))).toBe(false);
    expect(r.stdout).toContain("ffmpeg is not on PATH: no probe.mp4");
    // How to scrub it, frame by frame.
    expect(r.stdout).toContain("ui-recording-timeline/scripts/timeline.py");
    expect(r.stdout).toContain(`T extract '${join(out, "probe.mp4")}'`);
    expect(r.stdout).toContain("T candidates");
    expect(r.stdout).toContain("--frames 0,1,2 --name probe");
  });

  test("encodes_the_mp4_at_60_fps_when_ffmpeg_is_on_path", () => {
    writeFileSync(ffmpegLog, "");
    const out = join(sandbox.home, "probe-full");
    const r = record(["probe", "--out", out], {
      PATH: `${ffmpegBin}:${systemPath}`,
      FAKE_FRAMES: "16",
    });
    expect({ code: r.code, stderr: r.stderr }).toMatchObject({ code: 0 });
    expect(readFileSync(cargoLog, "utf8")).toContain(`dump=${out} policy=full`);
    const ffmpeg = readFileSync(ffmpegLog, "utf8");
    expect(ffmpeg).toContain("-framerate 60");
    expect(ffmpeg).toContain(`-i ${join(out, "frame-%04d.png")}`);
    expect(ffmpeg.trim().endsWith(join(out, "probe.mp4"))).toBe(true);
    expect(existsSync(join(out, "probe.mp4"))).toBe(true);
    // The strip's frames: the commit, the next three, ½ and the end.
    expect(r.stdout).toContain("--frames 0,1,2,3,7,15 --name probe");
  });

  test("stale frames are cleared, a run without frames fails, usage errors exit 2", () => {
    const out = join(sandbox.home, "stale");
    mkdirSync(out, { recursive: true });
    writeFileSync(join(out, "frame-0009.png"), "old");
    const fresh = record(["probe", "--out", out]);
    expect(fresh.code).toBe(0);
    expect(existsSync(join(out, "frame-0009.png"))).toBe(false);
    expect(fresh.stdout).toContain("3 frames");
    // A failing test still leaves its frames: reported, not fatal.
    const failing = record(["probe", "--out", out], { FAKE_EXIT: "101" });
    expect(failing.code).toBe(0);
    expect(failing.stderr).toContain("exited with 101");
    const none = record(["nothing", "--out", out], {
      FAKE_FRAMES: "0",
      FAKE_EXIT: "4",
    });
    expect(none.code).toBe(1);
    expect(none.stderr).toContain("e2e_motion_nothing wrote no frames");
    expect(record([]).code).toBe(2);
    expect(record(["probe", "--policy", "off"]).code).toBe(2);
    expect(record(["probe", "--frobnicate"]).code).toBe(2);
  });
});
