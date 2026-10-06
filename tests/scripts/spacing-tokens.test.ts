// The spacing lint (ADR-0031 "Where tokens live and how they are enforced",
// design §11.17, plan T7.1): every layout dimension in the app's and the
// viewport's sources comes from `space`. The scan strips comments, string
// contents, `#[cfg(test)]` items and the token modules (both `space.rs`
// files; the `tokens` and `ink` modules of the motion modules), then applies
// seven rules to what is left. Identifiers are matched by `_`-separated
// component. During M7 a per-file debt map ratchets each file's count down
// (`spacing-debt.json`; `UPDATE_SPACING_DEBT=1` re-seeds it and fails if an
// entry rose); T7.15 deletes it.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..");
const fixtureDir = join(import.meta.dir, "fixtures/spacing");
const debtPath = join(import.meta.dir, "spacing-debt.json");
const update = process.env.UPDATE_SPACING_DEBT === "1";

// ---------------------------------------------------------------------------
// The scanner

type Finding = {
  path: string;
  line: number;
  rule: number;
  snippet: string;
  group: string;
};

// Token modules: skipped whole, or their `tokens` and `ink` modules.
const SPACE_FILES = [
  "crates/polygloss-viewport/src/space.rs",
  "crates/polygloss-app/src/space.rs",
];
const MOTION_TOKEN_FILE =
  /^crates\/polygloss-(?:app|viewport)\/src\/motion\/(?:tokens|ink)\.rs$/;
const MOTION_MODULE = /^crates\/polygloss-(?:app|viewport)\/src\/motion\.rs$/;
const MOTION_TOKEN_MODULES = new Set(["tokens", "ink"]);
const SOURCE_DIRS = [
  "crates/polygloss-app/src",
  "crates/polygloss-viewport/src",
];

type Token = {
  kind: "ident" | "num" | "str" | "punct";
  text: string;
  start: number;
  end: number;
};

// Longest first; `<<` and `>>` stay apart (generics).
const PUNCT = [
  "..=",
  "...",
  "::",
  "->",
  "=>",
  "==",
  "!=",
  "<=",
  ">=",
  "&&",
  "||",
  "+=",
  "-=",
  "*=",
  "/=",
  "%=",
  "^=",
  "&=",
  "|=",
  "..",
];
const IDENT_START = /[A-Za-z_]/;
const IDENT = /[A-Za-z0-9_]/;
// r"…", r#"…"#, br#"…"#: the prefix and its hashes.
const RAW_STRING = /[bc]?r(#*)"/y;

// Rust tokens of `src`, without comments; strings and chars are one `str`
// token each, lifetimes an `ident`.
function lex(src: string): Token[] {
  const tokens: Token[] = [];
  const n = src.length;
  let i = 0;
  const push = (kind: Token["kind"], end: number) => {
    tokens.push({ kind, text: src.slice(i, end), start: i, end });
    i = end;
  };
  const quoted = (from: number) => {
    // `from` is the opening quote; returns the index after the closing one.
    let j = from + 1;
    while (j < n && src[j] !== '"') j += src[j] === "\\" ? 2 : 1;
    return j + 1;
  };
  while (i < n) {
    const c = src[i]!;
    RAW_STRING.lastIndex = i;
    const raw = RAW_STRING.exec(src);
    if (/\s/.test(c)) {
      i++;
    } else if (src.startsWith("//", i)) {
      const nl = src.indexOf("\n", i);
      i = nl < 0 ? n : nl;
    } else if (src.startsWith("/*", i)) {
      // Block comments nest.
      let depth = 0;
      do {
        if (src.startsWith("/*", i)) {
          depth++;
          i += 2;
        } else if (src.startsWith("*/", i)) {
          depth--;
          i += 2;
        } else {
          i++;
        }
      } while (i < n && depth > 0);
    } else if (raw) {
      const close = `"${raw[1]}`;
      const end = src.indexOf(close, i + raw[0].length);
      push("str", end < 0 ? n : end + close.length);
    } else if (c === '"' || (/[bc]/.test(c) && src[i + 1] === '"')) {
      push("str", quoted(c === '"' ? i : i + 1));
    } else if (c === "'" || (c === "b" && src[i + 1] === "'")) {
      const q = c === "'" ? i : i + 1;
      if (src[q + 1] === "\\") {
        let j = q + 3;
        while (j < n && src[j] !== "'") j++;
        push("str", j + 1);
      } else {
        const width = (src.codePointAt(q + 1) ?? 0) > 0xffff ? 2 : 1;
        if (src[q + 1 + width] === "'") {
          push("str", q + 2 + width);
        } else {
          // A lifetime or label.
          let j = q + 1;
          while (j < n && IDENT.test(src[j]!)) j++;
          push("ident", j);
        }
      }
    } else if (IDENT_START.test(c)) {
      let j = i + 1;
      while (j < n && IDENT.test(src[j]!)) j++;
      push("ident", j);
    } else if (/[0-9]/.test(c)) {
      push("num", numberEnd(src, i, tokens.at(-1)?.text === "."));
    } else {
      const op = PUNCT.find((p) => src.startsWith(p, i));
      push("punct", i + (op?.length ?? 1));
    }
  }
  return tokens;
}

// Where the number literal at `i` ends. After a lone `.` it is a tuple
// field (`pair.0`): digits only.
function numberEnd(src: string, i: number, field: boolean): number {
  let j = i;
  const at = (k: number) => src[k] ?? "";
  if (/^0[xob]/.test(src.slice(i, i + 2))) {
    j += 2;
    while (/[0-9a-fA-F_]/.test(at(j))) j++;
  } else {
    while (/[0-9_]/.test(at(j))) j++;
    if (field) return j;
    // `12.` and `12.5` are floats; `0..4`, `1.max(2)` are not.
    if (at(j) === "." && at(j + 1) !== "." && !IDENT_START.test(at(j + 1))) {
      j++;
      while (/[0-9_]/.test(at(j))) j++;
    }
    if (/[eE]/.test(at(j)) && /[0-9+-]/.test(at(j + 1))) {
      j += 2;
      while (/[0-9_]/.test(at(j))) j++;
    }
  }
  while (IDENT.test(at(j))) j++; // a suffix: f32, usize
  return j;
}

function numValue(text: string): number {
  const t = text.replace(/_/g, "");
  if (/^0[xob]/.test(t))
    return Number.parseInt(
      t.slice(2),
      { x: 16, o: 8, b: 2 }[t[1] as "x" | "o" | "b"],
    );
  return Number.parseFloat(t);
}

function isFloat(text: string): boolean {
  return !/^0[xob]/.test(text) && /[.eE]|f32$|f64$/.test(text);
}

const OPEN = new Set(["(", "[", "{"]);
const CLOSE = new Set([")", "]", "}"]);

// The index of the bracket closing the one at `open` (the last token if
// none does).
function matchClose(tokens: Token[], open: number): number {
  let depth = 0;
  for (let j = open; j < tokens.length; j++) {
    const t = tokens[j]!.text;
    if (OPEN.has(t)) depth++;
    else if (CLOSE.has(t) && --depth === 0) return j;
  }
  return tokens.length - 1;
}

// The index after the item starting at `j`: through its `;`, or through
// the block that closes it.
function itemEnd(tokens: Token[], j: number): number {
  while (j < tokens.length) {
    const t = tokens[j]!.text;
    if (t === ";") return j + 1;
    if (t === "{") return matchClose(tokens, j) + 1;
    j = t === "(" || t === "[" ? matchClose(tokens, j) + 1 : j + 1;
  }
  return j;
}

const CFG_TEST = ["#", "[", "cfg", "(", "test", ")", "]"];

// `tokens` without `#[cfg(test)]` items (their other attributes included).
function stripCfgTest(tokens: Token[]): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < tokens.length) {
    if (CFG_TEST.every((t, k) => tokens[i + k]?.text === t)) {
      let j = i + CFG_TEST.length;
      while (tokens[j]?.text === "#" && tokens[j + 1]?.text === "[") {
        j = matchClose(tokens, j + 1) + 1;
      }
      i = itemEnd(tokens, j);
    } else {
      out.push(tokens[i++]!);
    }
  }
  return out;
}

// `tokens` without the blocks of `mod <name> { … }` for `names`.
function stripModules(tokens: Token[], names: Set<string>): Token[] {
  const out: Token[] = [];
  let i = 0;
  while (i < tokens.length) {
    const t = tokens[i]!;
    if (
      t.text === "mod" &&
      names.has(tokens[i + 1]?.text ?? "") &&
      tokens[i + 2]?.text === "{"
    ) {
      i = matchClose(tokens, i + 2) + 1;
    } else {
      out.push(t);
      i++;
    }
  }
  return out;
}

// Rule 2: gpui's rem helpers, `{prefix}[_neg]_{suffix}`.
const BOX_PREFIXES = [
  "m",
  "mt",
  "mb",
  "my",
  "mx",
  "ml",
  "mr",
  "p",
  "pt",
  "pb",
  "px",
  "py",
  "pl",
  "pr",
  "inset",
  "top",
  "bottom",
  "left",
  "right",
  "w",
  "h",
  "size",
  "min_size",
  "min_w",
  "min_h",
  "max_size",
  "max_w",
  "max_h",
  "gap",
  "gap_x",
  "gap_y",
];
const ROUNDED_PREFIXES = [
  "rounded",
  "rounded_t",
  "rounded_b",
  "rounded_r",
  "rounded_l",
  "rounded_tl",
  "rounded_tr",
  "rounded_bl",
  "rounded_br",
];
const BORDER_PREFIXES = [
  "border",
  "border_t",
  "border_b",
  "border_r",
  "border_l",
  "border_x",
  "border_y",
];
const TEXT_PRESETS = new Set([
  "text_xs",
  "text_sm",
  "text_base",
  "text_lg",
  "text_xl",
  "text_2xl",
  "text_3xl",
]);

// The suffix of `name` after `prefix` (component by component), or null.
function suffixAfter(name: string, prefix: string): string[] | null {
  const parts = name.split("_");
  const head = prefix.split("_");
  return head.every((h, k) => parts[k] === h) ? parts.slice(head.length) : null;
}

// A rem helper that sets a dimension other than 0, full, auto, a fraction
// or the 1 pt border.
function isRemHelper(name: string): boolean {
  for (const prefix of BOX_PREFIXES) {
    let rest = suffixAfter(name, prefix);
    if (rest?.[0] === "neg") rest = rest.slice(1);
    const suffix = rest?.join("_");
    if (
      suffix !== undefined &&
      (/^\d+(?:p5)?$/.test(suffix) || suffix === "px")
    ) {
      if (suffix !== "0") return true;
    }
  }
  for (const prefix of ROUNDED_PREFIXES) {
    const suffix = suffixAfter(name, prefix)?.join("_");
    if (suffix !== undefined && /^(?:xs|sm|md|lg|xl|2xl|3xl)$/.test(suffix))
      return true;
  }
  for (const prefix of BORDER_PREFIXES) {
    const suffix = suffixAfter(name, prefix)?.join("_");
    if (
      suffix !== undefined &&
      /^\d+$/.test(suffix) &&
      suffix !== "0" &&
      suffix !== "1"
    )
      return true;
  }
  return false;
}

// Rules 4 and 5.
const GEOMETRY_TYPES = new Set(["f32", "Pixels", "Size", "Point"]);
const GEOMETRY_FIELDS = new Set([
  "margin",
  "gap",
  "radius",
  "pad",
  "padding",
  "height",
  "width",
  "inset",
  "offset",
  "border",
  "size",
  "indent",
]);
// Literals a binding or a call may hold (rules 1 and 4).
const SMALL = new Set([0, 1, 2, 0.5]);

// The token group a name points at, for failure messages.
function groupFor(name: string): string {
  const n = name.toLowerCase();
  if (
    /^(?:m[tbxylr]?|p[tbxylr]?|inset|top|bottom|left|right|margin.*|padding.*)$/.test(
      n,
    )
  )
    return "edge";
  if (/^(?:w|min_w|max_w|size|min_size|max_size)$/.test(n)) return "size";
  if (/^(?:h|min_h|max_h)$/.test(n)) return "height";
  if (/^gap/.test(n)) return "gap";
  if (/^rounded/.test(n)) return "radius";
  if (/^border/.test(n)) return "stroke";
  if (/^(?:text_.*|line_height)$/.test(n)) return "text";
  const parts = new Set(n.split("_"));
  for (const [words, group] of [
    [["radius", "rounded", "corner"], "radius"],
    [["border", "stroke"], "stroke"],
    [["gap"], "gap"],
    [["pad", "padding"], "pad"],
    [["margin", "inset", "offset", "edge"], "edge"],
    [["height"], "height"],
    [["width", "size", "indent", "icon", "avatar", "dot"], "size"],
  ] as const) {
    if (words.some((w) => parts.has(w))) return group;
  }
  return "*";
}

// The name of the call whose arguments hold token `i` (`.gap(` → `gap`).
function enclosingCall(tokens: Token[], i: number): string {
  let depth = 0;
  for (let j = i - 1; j >= 0; j--) {
    const t = tokens[j]!.text;
    if (CLOSE.has(t)) depth++;
    else if (OPEN.has(t)) {
      if (depth === 0) {
        return t === "(" && tokens[j - 1]?.kind === "ident"
          ? tokens[j - 1]!.text
          : "";
      }
      depth--;
    }
  }
  return "";
}

function scanSource(path: string, src: string): Finding[] {
  if (SPACE_FILES.includes(path) || MOTION_TOKEN_FILE.test(path)) return [];
  let tokens = stripCfgTest(lex(src));
  if (MOTION_MODULE.test(path))
    tokens = stripModules(tokens, MOTION_TOKEN_MODULES);
  const lineStarts = [0];
  for (let k = 0; k < src.length; k++)
    if (src[k] === "\n") lineStarts.push(k + 1);
  const lineOf = (offset: number) => {
    let lo = 0;
    let hi = lineStarts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (lineStarts[mid]! <= offset) lo = mid;
      else hi = mid - 1;
    }
    return lo + 1;
  };
  const covered = new Set<number>();
  const findings: Finding[] = [];
  const t = (k: number) => tokens[k];
  const text = (k: number) => tokens[k]?.text ?? "";
  const isNum = (k: number) => tokens[k]?.kind === "num";
  // A literal, maybe negative, filling tokens [from, to).
  const literal = (from: number, to: number) =>
    (to - from === 1 && isNum(from)) ||
    (to - from === 2 && text(from) === "-" && isNum(from + 1));
  // Tokens [from, to] as one line of code, without comments: a gap with a
  // line break closes up, except between two words.
  const span = (from: number, to: number) => {
    let out = text(from);
    for (let k = from + 1; k <= to; k++) {
      const gap = src.slice(t(k - 1)!.end, t(k)!.start);
      const words = /\w$/.test(text(k - 1)) && /^\w/.test(text(k));
      out +=
        (gap === "" ? "" : gap.includes("\n") && !words ? "" : " ") + text(k);
    }
    return out;
  };
  const add = (
    rule: number,
    from: number,
    to: number,
    group: string,
    snippet?: string,
  ) => {
    for (let k = from; k <= to; k++) covered.add(k);
    findings.push({
      path,
      line: lineOf(t(from)!.start),
      rule,
      snippet: snippet ?? span(from, to),
      group,
    });
  };
  // Skips one bracketed group or one token from `k`.
  const step = (k: number) =>
    OPEN.has(text(k)) ? matchClose(tokens, k) + 1 : k + 1;

  // Rule 4: numeric consts and statics of geometry types, and `let`
  // bindings of a literal.
  for (let i = 0; i < tokens.length; i++) {
    if ((text(i) === "const" || text(i) === "static") && text(i + 1) !== "fn") {
      const name = text(i + 1) === "mut" ? i + 2 : i + 1;
      if (t(name)?.kind !== "ident" || text(name + 1) !== ":") continue;
      let eq = name + 2;
      while (eq < tokens.length && text(eq) !== "=" && text(eq) !== ";")
        eq = step(eq);
      const type = tokens.slice(name + 2, eq);
      if (text(eq) !== "=" || !type.some((x) => GEOMETRY_TYPES.has(x.text)))
        continue;
      let end = eq + 1;
      while (end < tokens.length && text(end) !== ";") end = step(end);
      if (tokens.slice(eq + 1, end).some((x) => x.kind === "num")) {
        add(4, i, end - 1, groupFor(text(name)));
      }
    } else if (text(i) === "let") {
      const name = text(i + 1) === "mut" ? i + 2 : i + 1;
      if (t(name)?.kind !== "ident") continue;
      let eq = name + 1;
      if (text(eq) === ":")
        while (eq < tokens.length && text(eq) !== "=" && text(eq) !== ";")
          eq = step(eq);
      if (text(eq) !== "=") continue;
      let end = eq + 1;
      while (end < tokens.length && text(end) !== ";") end = step(end);
      if (literal(eq + 1, end) && !SMALL.has(numValue(text(end - 1)))) {
        add(4, i, end - 1, groupFor(text(name)));
      }
    }
  }

  // Rule 5: a struct-literal field with a geometry name set to a number
  // (0, as in rules 1 and 2, is none).
  for (let i = 1; i < tokens.length; i++) {
    if (t(i)!.kind !== "ident" || text(i + 1) !== ":" || covered.has(i))
      continue;
    if (text(i - 1) !== "{" && text(i - 1) !== ",") continue;
    const lit = text(i + 2) === "-" ? i + 3 : i + 2;
    if (!isNum(lit) || numValue(text(lit)) === 0) continue;
    if (text(lit + 1) !== "," && text(lit + 1) !== "}") continue;
    if (
      text(i)
        .toLowerCase()
        .split("_")
        .some((p) => GEOMETRY_FIELDS.has(p))
    ) {
      add(5, i, lit, groupFor(text(i)));
    }
  }

  // Rule 3: a line height outside the text helper.
  for (let i = 0; i < tokens.length; i++) {
    if (text(i) !== "line_height" || text(i + 1) !== "(") continue;
    if (text(i - 1) === "fn" || covered.has(i)) continue;
    add(3, text(i - 1) === "." ? i - 1 : i, matchClose(tokens, i + 1), "text");
  }

  // Rule 1: literal dimensions in px, rems, relative, Pixels and `.into()`.
  for (let i = 0; i < tokens.length; i++) {
    if (covered.has(i)) continue;
    const prev = text(i - 1);
    let open = -1;
    if (
      ["px", "rems", "relative", "Pixels"].includes(text(i)) &&
      text(i + 1) === "("
    ) {
      if (prev !== "." && prev !== "fn") open = i + 1;
    } else if (
      text(i) === "Pixels" &&
      text(i + 1) === "::" &&
      text(i + 2) === "from" &&
      text(i + 3) === "("
    ) {
      open = i + 3;
    } else if (
      isNum(i) &&
      text(i + 1) === "." &&
      text(i + 2) === "into" &&
      text(i + 3) === "(" &&
      text(i + 4) === ")"
    ) {
      if (numValue(text(i)) !== 0)
        add(1, i, i + 4, groupFor(enclosingCall(tokens, i)));
      continue;
    }
    if (open < 0) continue;
    const close = matchClose(tokens, open);
    const whole = literal(open + 1, close);
    const flagged = whole
      ? numValue(text(close - 1)) !== 0
      : tokens
          .slice(open + 1, close)
          .some((x) => x.kind === "num" && !SMALL.has(numValue(x.text)));
    if (flagged) add(1, i, close, groupFor(enclosingCall(tokens, i)));
  }

  // Rules 2 and 3: rem helpers and text presets.
  for (let i = 1; i < tokens.length; i++) {
    if (text(i - 1) !== "." || text(i + 1) !== "(" || text(i + 2) !== ")")
      continue;
    if (isRemHelper(text(i))) add(2, i - 1, i + 2, groupFor(text(i)));
    else if (TEXT_PRESETS.has(text(i))) add(3, i - 1, i + 2, "text");
  }

  // Rule 6: float literals in every viewport file.
  if (path.startsWith("crates/polygloss-viewport/src/")) {
    const exempt = new Set<number>();
    for (let i = 0; i < tokens.length; i++) {
      const color = ["hsla", "rgba"].includes(text(i)) && text(i - 1) !== ".";
      const alpha =
        ["alpha", "opacity"].includes(text(i)) && text(i - 1) === ".";
      if ((color || alpha) && text(i + 1) === "(") {
        const close = matchClose(tokens, i + 1);
        for (let k = i + 1; k <= close; k++) exempt.add(k);
      }
    }
    for (let i = 0; i < tokens.length; i++) {
      if (!isNum(i) || !isFloat(text(i)) || covered.has(i) || exempt.has(i))
        continue;
      const v = numValue(text(i));
      if (v === 0 || v === 1) continue;
      if (v === 2 && ["/", "/="].includes(text(i - 1))) continue;
      if (
        v === 0.5 &&
        (["*", "*="].includes(text(i - 1)) || text(i + 1) === "*")
      )
        continue;
      // The snippet is the literal's line.
      const line = lineOf(t(i)!.start);
      let first = i;
      let last = i;
      while (first > 0 && lineOf(t(first - 1)!.start) === line) first--;
      while (last + 1 < tokens.length && lineOf(t(last + 1)!.start) === line)
        last++;
      add(6, i, i, groupFor(enclosingCall(tokens, i)), span(first, last));
    }
  }

  // Rule 7: a Button without its ladder size, or extra-small at the kit
  // radius.
  for (let i = 0; i < tokens.length; i++) {
    if (
      text(i) !== "Button" ||
      text(i + 1) !== "::" ||
      text(i + 2) !== "new" ||
      text(i + 3) !== "("
    )
      continue;
    let end = matchClose(tokens, i + 3);
    while (
      text(end + 1) === "." &&
      t(end + 2)?.kind === "ident" &&
      text(end + 3) === "("
    ) {
      end = matchClose(tokens, end + 3);
    }
    const chain = tokens
      .slice(i, end + 1)
      .map((x) => x.text)
      .join("");
    const sized = /\.(?:x?small)\(\)/.test(chain);
    const xs = chain.includes(".xsmall()");
    const xsRadius = /\.rounded\(px\((?:space::)?radius::XS\)\)/.test(chain);
    if (!sized || (xs && !xsRadius)) {
      const snippet = span(i, end);
      add(
        7,
        i,
        end,
        sized ? "radius" : "height",
        snippet.length > 72 ? `${snippet.slice(0, 71)}…` : snippet,
      );
    }
  }

  return findings.sort((a, b) => a.line - b.line || a.rule - b.rule);
}

// Every `.rs` file under the scanned source dirs that git lists, tracked
// or untracked (ignored files excluded), as `rg --files` would.
function listSources(root: string): string[] {
  const r = Bun.spawnSync(
    [
      "git",
      "ls-files",
      "-z",
      "--cached",
      "--others",
      "--exclude-standard",
      "--",
      ...SOURCE_DIRS,
    ],
    { cwd: root, env: gitEnv },
  );
  if (r.exitCode !== 0)
    throw new Error(`git ls-files failed: ${r.stderr.toString()}`);
  return r.stdout
    .toString()
    .split("\0")
    .filter((p) => p.endsWith(".rs") && existsSync(join(root, p)));
}

// Every `pub const` of each top-level group module of a `space.rs` file
// that its group's `ALL` does not name, and groups with no `ALL`.
function checkGroups(path: string, src: string): string[] {
  const tokens = stripCfgTest(lex(src));
  const problems: string[] = [];
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i]!.text !== "mod" || tokens[i + 2]?.text !== "{") continue;
    const group = tokens[i + 1]!.text;
    const close = matchClose(tokens, i + 2);
    const body = tokens.slice(i + 3, close);
    const consts: string[] = [];
    let all: string[] | null = null;
    for (let k = 0; k < body.length; k++) {
      if (
        body[k]!.text !== "pub" ||
        body[k + 1]?.text !== "const" ||
        body[k + 2]?.text === "fn"
      )
        continue;
      const name = body[k + 2]!.text;
      if (name !== "ALL") {
        consts.push(name);
        continue;
      }
      let end = k + 3;
      while (end < body.length && body[end]!.text !== ";") end++;
      all = body
        .slice(k + 3, end)
        .filter((x) => x.kind === "str")
        .map((x) => x.text.slice(1, -1));
    }
    if (all === null) problems.push(`${path}: space::${group} has no ALL`);
    for (const name of consts) {
      if (all !== null && !all.includes(name)) {
        problems.push(
          `${path}: space::${group}::${name} is missing from space::${group}::ALL`,
        );
      }
    }
    i = close;
  }
  return problems;
}

function message(f: Finding): string {
  return `${f.path}:${f.line} rule ${f.rule} '${f.snippet}' → space::${f.group} (ADR-0031)`;
}

// Path + exact snippet + reason; a path ending in `/` is a directory, and
// an entry without a snippet allows the whole path.
type Allow = { path: string; snippet?: string; reason: string };

const ALLOW: Allow[] = [
  {
    path: "crates/polygloss-app/src/perf/",
    reason: "the perf harness's scenarios (test-only), not UI",
  },
  {
    path: "crates/polygloss-app/src/dump.rs",
    reason: "the hidden keymap and settings dumps print text, no layout",
  },
  {
    path: "crates/polygloss-viewport/src/layout.rs",
    snippet: "const AUTO_LAYOUT_HYSTERESIS_COLUMNS: f32 = 8.0",
    reason: "code columns, not points",
  },
  {
    path: "crates/polygloss-viewport/src/pipeline.rs",
    snippet: "const CANCEL_SLACK_SCREENS: f32 = 1.0",
    reason: "viewport heights, not points",
  },
  {
    path: "crates/polygloss-viewport/src/blocks.rs",
    snippet: "const ESTIMATED_BLOCK_ROWS: f32 = 3.0",
    reason: "code rows, not points",
  },
  {
    path: "crates/polygloss-viewport/src/document/window.rs",
    snippet: "const DEFAULT_WINDOW_SCREENS: f32 = 2.0",
    reason: "viewport heights, not points",
  },
  {
    path: "crates/polygloss-viewport/src/style.rs",
    snippet: "const SHARE: f32 = 0.1",
    reason: "a color-mix share, not a dimension",
  },
  {
    path: "crates/polygloss-viewport/src/header.rs",
    snippet: "const MIN_TITLE_COLUMNS: f32 = 12.0",
    reason: "code columns, not points",
  },
  {
    path: "crates/polygloss-viewport/src/special.rs",
    snippet: "let mut value = bytes as f64 / 1024.0;",
    reason: "byte units (KiB) in a file size, not points",
  },
  {
    path: "crates/polygloss-viewport/src/special.rs",
    snippet: "while value >= 1023.95 && unit + 1 < UNITS.len() {",
    reason: "byte units (KiB) in a file size, not points",
  },
  {
    path: "crates/polygloss-viewport/src/special.rs",
    snippet: "value /= 1024.0;",
    reason: "byte units (KiB) in a file size, not points",
  },
  {
    path: "crates/polygloss-viewport/src/layout.rs",
    snippet: "const DEFAULT_CODE_FONT_SIZE: f32 = 13.0",
    reason: "the code font's default size, a user setting, not layout",
  },
  {
    path: "crates/polygloss-viewport/src/layout.rs",
    snippet: "0.6 * font_size",
    reason:
      "a monospace advance's usual share of its size: the fallback when the font reports none",
  },
  {
    path: "crates/polygloss-viewport/src/cursor.rs",
    snippet:
      "let margin = (2.0 * row_h).min(((view_h - header) / 2.0 - row_h).max(0.0));",
    reason: "a scroll margin of two code rows, not points",
  },
  {
    path: "crates/polygloss-viewport/src/cursor.rs",
    snippet: "top - (vis_top + (view_h - header) / 3.0).floor()",
    reason: "a jump lands a third of the way down: a ratio, not a dimension",
  },
  {
    path: "crates/polygloss-viewport/src/selection.rs",
    snippet: "y - (c.y + c.h) + 0.5",
    reason: "a tie-break between equally distant cells, not a dimension",
  },
  {
    path: "crates/polygloss-viewport/src/document/window.rs",
    snippet: "let bottom = bottom.min(self.heights.total() - 0.5);",
    reason: "a probe half a point inside the document's end, not a dimension",
  },
  {
    path: "crates/polygloss-app/src/settings/model.rs",
    snippet: "size: 13.0",
    reason: "the code font's default size, a user setting, not layout",
  },
  {
    path: "crates/polygloss-viewport/src/element.rs",
    snippet: "relative(1.)",
    reason: "100% of the parent, a custom element's size_full",
  },
  {
    path: "crates/polygloss-app/src/review_tab/toolbar.rs",
    snippet: "relative(1.)",
    reason: "100% of the parent, a custom element's size_full",
  },
  {
    path: "crates/polygloss-app/src/composer/draft_store.rs",
    snippet: "let mut h: u64 = 0xcbf2_9ce4_8422_2325",
    reason: "the FNV-1a offset basis, a hash seed",
  },
  {
    path: "crates/polygloss-app/src/threads/placement.rs",
    snippet: "let mut h: u64 = 0xcbf2_9ce4_8422_2325",
    reason: "the FNV-1a offset basis, a hash seed",
  },
];

// The debt map for `counts` (files at 0 leave it), and every entry that
// rose (a file absent from `old` counts as 0).
function reseed(
  old: Record<string, number>,
  counts: Record<string, number>,
): { next: Record<string, number>; rises: string[] } {
  const next = Object.fromEntries(
    Object.entries(counts).filter(([, n]) => n > 0),
  );
  const rises = Object.entries(next)
    .filter(([path, n]) => n > (old[path] ?? 0))
    .map(([path, n]) => debtMessage(path, old[path] ?? 0, n));
  return { next, rises };
}

function debtMessage(path: string, from: number, to: number): string {
  return `spacing-debt.json[${path}]: ${from} → ${to}`;
}

// ---------------------------------------------------------------------------
// Fixtures: `// path: <repo path>` on the first line, then cases that each
// start with `// accept` or `// reject N`.

type Case = { header: string; expect: "accept" | number; source: string };

function fixture(name: string): { path: string; text: string } {
  const text = readFileSync(join(fixtureDir, name), "utf8");
  const path = /^\/\/ path: (\S+)/.exec(text)?.[1];
  if (!path) throw new Error(`${name}: no "// path:" line`);
  return { path, text };
}

function cases(text: string): Case[] {
  const out: Case[] = [];
  for (const line of text.split("\n")) {
    const header = /^\/\/ (accept|reject (\d))\b/.exec(line);
    if (header) {
      out.push({
        header: line,
        expect: header[2] ? Number(header[2]) : "accept",
        source: "",
      });
    } else if (out.length > 0) {
      out[out.length - 1]!.source += `${line}\n`;
    }
  }
  return out;
}

function scanFixture(name: string): Finding[] {
  const { path, text } = fixture(name);
  return scanSource(path, text);
}

// ---------------------------------------------------------------------------
// The tree: scanned once.

let gitEnv: Record<string, string> = {};
let sandbox = "";
let tree: {
  sources: string[];
  findings: Finding[];
  counts: Record<string, number>;
} = { sources: [], findings: [], counts: {} };

function lintTree(root: string): typeof tree {
  const sources = listSources(root);
  const findings = sources.flatMap((p) =>
    scanSource(p, readFileSync(join(root, p), "utf8")),
  );
  const kept = findings.filter((f) => !allowed(f));
  const counts: Record<string, number> = {};
  for (const f of kept) counts[f.path] = (counts[f.path] ?? 0) + 1;
  return { sources, findings, counts };
}

function allowed(f: Finding): boolean {
  return ALLOW.some(
    (a) =>
      (a.path.endsWith("/") ? f.path.startsWith(a.path) : f.path === a.path) &&
      (a.snippet === undefined || a.snippet === f.snippet),
  );
}

function readDebt(): Record<string, number> {
  return existsSync(debtPath)
    ? (JSON.parse(readFileSync(debtPath, "utf8")) as Record<string, number>)
    : {};
}

function writeDebt(map: Record<string, number>) {
  const sorted = Object.fromEntries(
    Object.entries(map).sort(([a], [b]) => (a < b ? -1 : 1)),
  );
  writeFileSync(debtPath, `${JSON.stringify(sorted, null, 2)}\n`);
}

beforeAll(() => {
  sandbox = realpathSync(mkdtempSync(join(tmpdir(), "polygloss-spacing-")));
  writeFileSync(join(sandbox, "gitconfig"), "");
  gitEnv = {
    HOME: sandbox,
    XDG_CONFIG_HOME: join(sandbox, ".config"),
    GIT_CONFIG_GLOBAL: join(sandbox, "gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
    PATH: process.env.PATH ?? "/usr/bin:/bin",
  };
  tree = lintTree(repoRoot);
});

afterAll(() => {
  if (sandbox) rmSync(sandbox, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------

describe("the spacing lint (ADR-0031)", () => {
  test("each rule accepts and rejects its fixtures", () => {
    const names = readdirSync(fixtureDir).filter(
      (n) => n.endsWith(".rs") && n !== "space_missing_all.rs",
    );
    const seen = new Set<number>();
    let accepted = 0;
    const wrong: string[] = [];
    for (const name of names) {
      const { path, text } = fixture(name);
      for (const c of cases(text)) {
        const found = scanSource(path, c.source);
        if (c.expect === "accept") {
          accepted++;
          if (found.length > 0) {
            wrong.push(`${name} ${c.header}: ${found.map(message).join("; ")}`);
          }
        } else {
          seen.add(c.expect);
          if (!found.some((f) => f.rule === c.expect)) {
            wrong.push(
              `${name} ${c.header}: rule ${c.expect} not found (${found.map(message).join("; ") || "nothing"})`,
            );
          }
        }
      }
    }
    expect(wrong).toEqual([]);
    expect([...seen].sort()).toEqual([1, 2, 3, 4, 5, 6, 7]);
    expect(accepted).toBeGreaterThan(20);
  });

  test("every_pub_const_is_in_its_groups_all", () => {
    const text = readFileSync(join(fixtureDir, "space_missing_all.rs"), "utf8");
    expect(checkGroups("fixture/space.rs", text)).toEqual([
      "fixture/space.rs: space::gap::ORPHAN is missing from space::gap::ALL",
      "fixture/space.rs: space::stroke has no ALL",
    ]);
    for (const path of [
      "crates/polygloss-viewport/src/space.rs",
      "crates/polygloss-app/src/space.rs",
    ]) {
      const src = readFileSync(join(repoRoot, path), "utf8");
      expect(checkGroups(path, src)).toEqual([]);
      // A real group module is checked, not skipped.
      expect(src).toMatch(/pub mod \w+ \{[^]*pub const ALL/);
    }
  });

  test("failures_name_path_line_rule_and_token_group", () => {
    const found = scanSource(
      "crates/polygloss-app/src/x.rs",
      "fn f() {\n    div()\n        .gap(px(12.))\n        .rounded_md();\n}\n",
    );
    expect(found.map(message)).toEqual([
      "crates/polygloss-app/src/x.rs:3 rule 1 'px(12.)' → space::gap (ADR-0031)",
      "crates/polygloss-app/src/x.rs:4 rule 2 '.rounded_md()' → space::radius (ADR-0031)",
    ]);
    expect(debtMessage("crates/polygloss-app/src/x.rs", 3, 4)).toBe(
      "spacing-debt.json[crates/polygloss-app/src/x.rs]: 3 → 4",
    );
  });

  test("untracked_files_are_scanned", () => {
    const root = join(sandbox, "untracked");
    const src = join(root, "crates/polygloss-app/src");
    mkdirSync(src, { recursive: true });
    const git = (...args: string[]) => {
      const r = Bun.spawnSync(["git", ...args], { cwd: root, env: gitEnv });
      if (r.exitCode !== 0) throw new Error(r.stderr.toString());
    };
    git("init", "-q");
    writeFileSync(join(root, ".gitignore"), "ignored.rs\n");
    writeFileSync(join(src, "tracked.rs"), "fn f() { div().w(px(12.)); }\n");
    git("add", "-A");
    writeFileSync(join(src, "untracked.rs"), "fn g() { div().h(px(13.)); }\n");
    writeFileSync(join(src, "ignored.rs"), "fn h() { div().h(px(14.)); }\n");
    writeFileSync(join(src, "notes.txt"), "px(15.)\n");
    expect(listSources(root).sort()).toEqual([
      "crates/polygloss-app/src/tracked.rs",
      "crates/polygloss-app/src/untracked.rs",
    ]);
    expect(lintTree(root).counts).toEqual({
      "crates/polygloss-app/src/tracked.rs": 1,
      "crates/polygloss-app/src/untracked.rs": 1,
    });
  });

  test("cfg_test_items_are_stripped_by_brace_matching", () => {
    const found = scanFixture("cfg_test.rs");
    expect(found.map((f) => f.snippet)).toEqual(["px(12.)", "px(15.)"]);
    // Lines are the source's: stripping keeps every line in place.
    const { text } = fixture("cfg_test.rs");
    const lines = text.split("\n");
    for (const f of found) expect(lines[f.line - 1]).toContain(f.snippet);
  });

  test("token_modules_are_exempt_and_motion_rendering_is_not", () => {
    const tokens = scanFixture("motion_tokens.rs");
    expect(tokens.map((f) => [f.rule, f.snippet])).toEqual([
      [6, "offset * 0.75"],
    ]);
    expect(scanFixture("motion_ink.rs")).toEqual([]);
    expect(scanFixture("space_exempt.rs")).toEqual([]);
    expect(scanFixture("motion_wrap.rs").map((f) => f.rule)).toEqual([4, 1]);
    // The same `tokens` module outside a motion module is scanned.
    const { text } = fixture("motion_tokens.rs");
    expect(
      scanSource("crates/polygloss-viewport/src/header.rs", text).length,
    ).toBeGreaterThan(1);
  });

  test("every_viewport_file_meets_rule_6", () => {
    // No list of painter files: a new viewport file is scanned for float
    // literals; the same code in an app file is a review item.
    const { text } = fixture("viewport_new_file.rs");
    expect(
      scanSource("crates/polygloss-viewport/src/never_listed.rs", text).map(
        (f) => f.rule,
      ),
    ).toEqual([6]);
    expect(
      scanSource("crates/polygloss-viewport/src/document/new_dir.rs", text)
        .length,
    ).toBe(1);
    expect(
      scanSource("crates/polygloss-app/src/never_listed.rs", text),
    ).toEqual([]);
  });

  test("allow_list_entries_are_not_stale", () => {
    const stale = ALLOW.filter((a) =>
      a.snippet === undefined
        ? !existsSync(join(repoRoot, a.path))
        : !tree.findings.some(
            (f) => f.path === a.path && f.snippet === a.snippet,
          ),
    ).map((a) => `${a.path} '${a.snippet ?? ""}' (${a.reason})`);
    expect(stale).toEqual([]);
    expect(ALLOW.every((a) => a.reason.length > 0)).toBe(true);
  });

  test("reseed_fails_when_an_entry_rises", () => {
    expect(reseed({ "a.rs": 3, "b.rs": 2 }, { "a.rs": 1, "b.rs": 2 })).toEqual({
      next: { "a.rs": 1, "b.rs": 2 },
      rises: [],
    });
    // A file that reaches 0 leaves the map.
    expect(reseed({ "a.rs": 3 }, {}).next).toEqual({});
    expect(reseed({ "a.rs": 3 }, { "a.rs": 4 }).rises).toEqual([
      "spacing-debt.json[a.rs]: 3 → 4",
    ]);
    // A file absent from the map counts as 0.
    expect(reseed({}, { "new.rs": 1 }).rises).toEqual([
      "spacing-debt.json[new.rs]: 0 → 1",
    ]);
  });
});

describe("the debt map (M7 only; T7.15 deletes it)", () => {
  test.if(update)("UPDATE_SPACING_DEBT=1 re-seeds the debt map", () => {
    if (!existsSync(debtPath)) {
      writeDebt(tree.counts); // the first seed (T7.1)
      return;
    }
    const { next, rises } = reseed(readDebt(), tree.counts);
    expect(rises).toEqual([]);
    writeDebt(next);
  });

  test("no_file_exceeds_its_debt", () => {
    const debt = readDebt();
    const over = Object.entries(tree.counts)
      .filter(([path, count]) => count > (debt[path] ?? 0))
      .flatMap(([path, count]) => [
        debtMessage(path, debt[path] ?? 0, count),
        ...tree.findings
          .filter((f) => f.path === path && !allowed(f))
          .map(message),
      ]);
    expect(over).toEqual([]);
  });

  test("no_file_is_below_its_debt", () => {
    const below = Object.entries(readDebt())
      .filter(([path, entry]) => (tree.counts[path] ?? 0) < entry)
      .map(([path, entry]) => debtMessage(path, entry, tree.counts[path] ?? 0));
    expect(below).toEqual([]);
  });

  test("the_debt_map_names_only_existing_files", () => {
    expect(existsSync(debtPath)).toBe(true);
    const missing = Object.keys(readDebt()).filter(
      (p) => !tree.sources.includes(p),
    );
    expect(missing).toEqual([]);
  });
});
