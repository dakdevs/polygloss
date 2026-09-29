// scripts/port-pierre-theme.ts (plan T2.2): ports @pierre/theme 2.x to Zed
// theme JSON. The port is deterministic and its committed output in
// assets/themes/ (compiled into polygloss-highlight) must match a fresh run.
import { afterAll, describe, expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { makeSandbox } from "../support/sandbox";

const repoRoot = resolve(import.meta.dir, "../..");
const script = join(repoRoot, "scripts/port-pierre-theme.ts");
const committedDir = join(repoRoot, "assets/themes");
const themeFiles = ["pierre-dark.json", "pierre-light.json"];

const sandbox = makeSandbox();
afterAll(() => sandbox.cleanup());

function run(args: string[]): { exitCode: number; stderr: string } {
  const r = Bun.spawnSync([process.execPath, script, ...args], {
    cwd: repoRoot,
    env: sandbox.env,
  });
  return { exitCode: r.exitCode, stderr: r.stderr.toString() };
}

function port(name: string): string {
  const outDir = join(sandbox.home, name);
  const r = run(["--out", outDir]);
  if (r.exitCode !== 0) {
    throw new Error(`port-pierre-theme exited ${r.exitCode}: ${r.stderr}`);
  }
  return outDir;
}

type ZedFamily = {
  $schema: string;
  name: string;
  themes: {
    name: string;
    appearance: string;
    style: Record<string, unknown> & {
      syntax: Record<
        string,
        { color: string; font_style?: string; font_weight?: number }
      >;
    };
  }[];
};

function readFamily({ dir, file }: { dir: string; file: string }): ZedFamily {
  return JSON.parse(readFileSync(join(dir, file), "utf8")) as ZedFamily;
}

describe("port-pierre-theme", () => {
  test("port output is stable", () => {
    const first = port("first");
    const second = port("second");
    expect(readdirSync(first).sort()).toEqual(themeFiles);
    for (const file of themeFiles) {
      expect(readFileSync(join(second, file), "utf8")).toBe(
        readFileSync(join(first, file), "utf8"),
      );
    }
  });

  test("committed themes are the current port output", () => {
    const fresh = port("fresh");
    for (const file of themeFiles) {
      expect(readFileSync(join(committedDir, file), "utf8")).toBe(
        readFileSync(join(fresh, file), "utf8"),
      );
    }
  });

  test("port maps UI, diff and syntax colors from @pierre/theme", () => {
    const dir = port("mapped");
    const light = readFamily({ dir, file: "pierre-light.json" });
    expect(light.$schema).toBe("https://zed.dev/schema/themes/v0.2.0.json");
    expect(light.name).toBe("Pierre Light");
    expect(light.themes).toHaveLength(1);
    const theme = light.themes[0]!;
    expect(theme.appearance).toBe("light");
    // UI colors (Pierre: editor.background #ffffff, focusBorder #009fff).
    expect(theme.style["editor.background"]).toBe("#ffffffff");
    expect(theme.style["border.focused"]).toBe("#009fffff");
    // Diff colors: line backgrounds from diffEditor.*TextBackground, words at
    // twice the alpha.
    expect(theme.style["created"]).toBe("#18a46cff");
    expect(theme.style["created.background"]).toBe("#18a46c33");
    expect(theme.style["deleted.background"]).toBe("#d52c3633");
    expect(theme.style["version_control.word_added"]).toBe("#18a46c66");
    expect(theme.style["version_control.word_deleted"]).toBe("#d52c3666");
    // Syntax: TextMate resolution (most specific selector, later rule wins ties).
    const syntax = theme.style.syntax;
    expect(syntax["keyword"]?.color).toBe("#d32a61ff");
    expect(syntax["string"]?.color).toBe("#199f43ff");
    expect(syntax["comment"]?.color).toBe("#737373ff");
    expect(syntax["operator"]?.color).toBe("#08c0efff");
    expect(syntax["variable.parameter"]?.color).toBe("#636363ff");
    expect(syntax["markup.italic"]?.font_style).toBe("italic");
    expect(syntax["markup.strong"]?.font_weight).toBe(700);
    expect(Object.keys(syntax)).toEqual(Object.keys(syntax).sort());

    const dark = readFamily({ dir, file: "pierre-dark.json" }).themes[0]!;
    expect(dark.name).toBe("Pierre Dark");
    expect(dark.appearance).toBe("dark");
    expect(dark.style["editor.background"]).toBe("#0a0a0aff");
    expect(dark.style["created.background"]).toBe("#07c4801a");
    expect(dark.style["version_control.word_added"]).toBe("#07c48034");
    expect(dark.style.syntax["keyword"]?.color).toBe("#ff678dff");
  });

  test("unknown arguments are rejected", () => {
    const r = run(["--bogus"]);
    expect(r.exitCode).toBe(2);
    expect(r.stderr).toContain("unknown argument: --bogus");
  });
});
