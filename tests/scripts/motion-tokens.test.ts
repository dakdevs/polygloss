// ADR-0030's motion rules that code review alone would miss (T7.2): one
// sampler on the executor clock (rule 5), motion state in its owner, never
// keyed element state (rule 13), and one writer of `App::reduce_motion`.

import { describe, expect, test } from "bun:test";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");

type Rule = { name: string; pattern: RegExp; why: string };

// gpui's wall-clock animations and gpui-base's motion module (rule 5).
const animationApi: Rule[] = [
  { name: "with_animation", pattern: /\bwith_animations?\b/, why: "rule 5" },
  { name: "with_spring", pattern: /\bwith_spring\b/, why: "rule 5" },
  { name: "Animation::new", pattern: /\bAnimation::new\b/, why: "rule 5" },
  { name: "base::motion", pattern: /\bbase::motion\b/, why: "rule 5" },
  { name: "MotionReveal", pattern: /\bMotionReveal\b/, why: "rule 5" },
  { name: "Presence", pattern: /\bPresence\w*\b/, why: "rule 5" },
  {
    name: "animate_keyframes",
    pattern: /\banimate_keyframes\b/,
    why: "rule 5",
  },
];

// The executor clock only, so tests step motion (rule 5).
const wallClock: Rule[] = [
  { name: "Instant::now", pattern: /\bInstant::now\b/, why: "rule 5" },
  { name: "SystemTime::now", pattern: /\bSystemTime::now\b/, why: "rule 5" },
];

// A `Track` field of the owning entity (rule 13).
const keyedState: Rule[] = [
  { name: "use_keyed_state", pattern: /\buse_keyed_state\b/, why: "rule 13" },
  { name: "use_state", pattern: /\buse_state\b/, why: "rule 13" },
];

// gpui-base's `apply_system_reduce_motion` is the flag's only writer in
// production; harnesses go through `motion::set_override`.
const reduceMotionWrite: Rule[] = [
  {
    name: "set_reduce_motion",
    pattern: /\bset_reduce_motion\b/,
    why: "Motion policy",
  },
];

// The motion modules: the app's `motion.rs` and `motion/`, the viewport's
// `motion.rs` and `reveal.rs`.
const motionModules = [
  "crates/polygloss-app/src/motion.rs",
  "crates/polygloss-app/src/motion",
  "crates/polygloss-viewport/src/motion.rs",
  "crates/polygloss-viewport/src/reveal.rs",
];

/** Every `.rs` file under `path` (a file or a directory), repo-relative. */
function rustFiles(path: string): string[] {
  const full = join(repoRoot, path);
  if (!existsSync(full)) return [];
  if (statSync(full).isFile()) return path.endsWith(".rs") ? [path] : [];
  return readdirSync(full).flatMap((name) => rustFiles(join(path, name)));
}

/** `text` with `[start, end)` blanked, newlines kept (line numbers hold). */
function blank(text: string, start: number, end: number): string {
  return (
    text.slice(0, start) +
    text.slice(start, end).replace(/[^\n]/g, " ") +
    text.slice(end)
  );
}

/** Comments and string literals' contents blanked: only code is matched. */
function codeOnly(src: string): string {
  let out = src;
  let i = 0;
  while (i < out.length) {
    const c = out[i];
    if (c === "'" && out[i + 2] === "'") {
      i += 3; // a char literal such as '"'
    } else if (c === "'" && out[i + 1] === "\\" && out[i + 3] === "'") {
      i += 4; // an escaped one such as '\''
    } else if (c === '"') {
      const start = ++i;
      while (i < out.length && out[i] !== '"') i += out[i] === "\\" ? 2 : 1;
      out = blank(out, start, i);
      i++;
    } else if (c === "/" && out[i + 1] === "/") {
      const end = out.indexOf("\n", i);
      const stop = end === -1 ? out.length : end;
      out = blank(out, i, stop);
      i = stop;
    } else if (c === "/" && out[i + 1] === "*") {
      const end = out.indexOf("*/", i + 2);
      const stop = end === -1 ? out.length : end + 2;
      out = blank(out, i, stop);
      i = stop;
    } else {
      i++;
    }
  }
  return out;
}

/** The index just past the `}` closing the first `{` at or after `from`. */
function closingBrace(src: string, from: number): number {
  const open = src.indexOf("{", from);
  if (open === -1) return src.length;
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    if (src[i] === "{") depth++;
    if (src[i] === "}" && --depth === 0) return i + 1;
  }
  return src.length;
}

/** `#[cfg(test)]` items blanked by brace matching (`mod x;` alone too). */
function stripCfgTest(src: string): string {
  let out = src;
  let at = out.indexOf("#[cfg(test)]");
  while (at !== -1) {
    const semicolon = out.indexOf(";", at);
    const brace = out.indexOf("{", at);
    const end =
      semicolon !== -1 && (brace === -1 || semicolon < brace)
        ? semicolon + 1
        : closingBrace(out, at);
    out = blank(out, at, end);
    at = out.indexOf("#[cfg(test)]", end);
  }
  return out;
}

/** The body of `fn name` blanked (the one place a rule allows a match). */
function stripFn(src: string, name: string): string {
  const at = src.search(new RegExp(`\\bfn ${name}\\b`));
  return at === -1 ? src : blank(src, at, closingBrace(src, at));
}

/** `path:line rule 'snippet' (why)` for each match of `rules` in `src`. */
function violations(path: string, src: string, rules: Rule[]): string[] {
  const lines = stripCfgTest(codeOnly(src)).split("\n");
  return lines.flatMap((line, i) =>
    rules
      .filter((rule) => rule.pattern.test(line))
      .map(
        (rule) =>
          `${path}:${i + 1} ${rule.name} '${src.split("\n")[i]?.trim()}' (ADR-0030 ${rule.why})`,
      ),
  );
}

function scan(paths: string[], rules: Rule[]): string[] {
  return paths
    .flatMap(rustFiles)
    .flatMap((path) =>
      violations(path, readFileSync(join(repoRoot, path), "utf8"), rules),
    );
}

describe("fixtures per rule", () => {
  const cases: [Rule[], string[], string[]][] = [
    [
      animationApi,
      [
        "let now = cx.background_executor().now();",
        "// with_animation reads the wall clock",
        'let text = "with_spring";',
        "use gpui_kit::base::animation::cubic_bezier;",
        "#[cfg(test)]\nmod tests {\n    fn f() { div().with_animation(a) }\n}",
      ],
      [
        "div().with_animation(id, anim, f)",
        "div().with_animations(id, anims, f)",
        "x.with_spring(id, spring, f)",
        "let a = Animation::new(d);",
        "use gpui_kit::base::motion::{Keyframes};",
        "MotionReveal::new(id)",
        "let p = Presence::new(true);",
        "gpui_kit::base::animate_keyframes(id, &k, t, window, cx)",
      ],
    ],
    [
      wallClock,
      ["let now = cx.background_executor().now();", "track.sample(now)"],
      [
        "let now = Instant::now();",
        "let t = std::time::SystemTime::now();",
        "let quote = '\"'; let now = Instant::now();",
      ],
    ],
    [
      keyedState,
      ["self.track.sample(now)", "let used_state = 1;"],
      [
        "window.use_keyed_state(id, cx, |_, _| 0)",
        "window.use_state(cx, |_, _| 0)",
      ],
    ],
    [
      reduceMotionWrite,
      ["cx.reduce_motion()", "// set_reduce_motion is the harnesses' job"],
      ["cx.set_reduce_motion(true);"],
    ],
  ];
  for (const [rules, accepted, rejected] of cases) {
    const name = rules.map((r) => r.name).join(", ");
    test(`${name} accepts`, () => {
      for (const src of accepted)
        expect(violations("f.rs", src, rules)).toEqual([]);
    });
    test(`${name} rejects`, () => {
      for (const src of rejected) {
        expect(violations("f.rs", src, rules).length).toBe(1);
      }
    });
  }

  test("failures name the path, line, rule and snippet", () => {
    expect(
      violations("src/a.rs", "fn f() {}\nlet t = Instant::now();\n", wallClock),
    ).toEqual([
      "src/a.rs:2 Instant::now 'let t = Instant::now();' (ADR-0030 rule 5)",
    ]);
  });

  test("only the named function's body is exempt", () => {
    const src = [
      "pub fn set_override(cx: &mut App) {",
      "    cx.set_reduce_motion(true);",
      "}",
      "pub fn other(cx: &mut App) {",
      "    cx.set_reduce_motion(false);",
      "}",
    ].join("\n");
    expect(
      violations("m.rs", stripFn(src, "set_override"), reduceMotionWrite),
    ).toEqual([
      "m.rs:5 set_reduce_motion 'cx.set_reduce_motion(false);' (ADR-0030 Motion policy)",
    ]);
  });
});

describe("the tree", () => {
  test("no_gpui_animation_api_in_app_or_viewport_src", () => {
    const paths = ["crates/polygloss-app/src", "crates/polygloss-viewport/src"];
    expect(paths.flatMap(rustFiles).length).toBeGreaterThan(50);
    expect(scan(paths, animationApi)).toEqual([]);
  });

  test("no_wall_clock_in_motion_modules", () => {
    expect(motionModules.flatMap(rustFiles).length).toBeGreaterThanOrEqual(5);
    expect(scan(motionModules, wallClock)).toEqual([]);
  });

  test("no_keyed_state_in_motion_modules", () => {
    expect(scan(motionModules, keyedState)).toEqual([]);
  });

  test("production_never_writes_reduce_motion", () => {
    const crates = readdirSync(join(repoRoot, "crates")).map((c) =>
      join("crates", c, "src"),
    );
    const found = crates.flatMap(rustFiles).flatMap((path) => {
      let src = readFileSync(join(repoRoot, path), "utf8");
      if (path === "crates/polygloss-app/src/motion.rs") {
        src = stripFn(src, "set_override");
      }
      return violations(path, src, reduceMotionWrite);
    });
    expect(found).toEqual([]);
  });
});
