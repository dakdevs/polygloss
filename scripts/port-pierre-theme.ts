#!/usr/bin/env bun
// Ports @pierre/theme 2.x (Apache-2.0, VS Code theme JSON) to Zed theme-family
// JSON, the format Polygloss loads (design §11.10, ADR-0024):
//
//   - UI colors go into `style` under Zed's keys;
//   - token colors go into `style.syntax`, under Zed's capture names and the
//     nvim-treesitter capture names lumis reports (matched by longest dotted
//     prefix in polygloss-highlight, so only distinct colors need a key);
//   - diff colors go into `created`/`deleted`/`modified` (+ `.background`,
//     `.border`), `version_control.*`, and the word highlights
//     `version_control.word_added`/`version_control.word_deleted`.
//
// Token colors are resolved the way VS Code and Shiki resolve TextMate scopes
// (the most specific matching selector wins, later rules win ties), so a capture
// gets the color diffs.com shows for the equivalent TextMate scope. lumis and
// Shiki tokenize differently, so the capture → scope table is tuned by hand.
//
// Deterministic and re-runnable; the output in assets/themes/ is committed and
// compiled into polygloss-highlight.
//
//   bun scripts/port-pierre-theme.ts [--out <dir>]    (default: assets/themes)
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { format, resolveConfig } from "prettier";

type TokenRule = {
  scope?: string | string[];
  settings: { foreground?: string; fontStyle?: string };
};
type VsTheme = {
  name: string;
  displayName: string;
  type: "light" | "dark";
  colors: Record<string, string>;
  tokenColors: TokenRule[];
};
type SyntaxEntry = {
  color: string;
  font_style?: "italic";
  font_weight?: number;
};

const repoRoot = resolve(import.meta.dir, "..");
const variants = ["light", "dark"] as const;

// Capture → the TextMate scope whose Pierre color it takes. Zed's standard
// capture names come first, then the nvim-treesitter names lumis emits where
// they need a color of their own. Captures not listed fall back by prefix
// (`keyword.function` → `keyword`), which is how every `keyword.*` stays
// Pierre's keyword pink.
const captures: [capture: string, textMateScope: string][] = [
  // Zed's names (also the base of most lumis names).
  ["attribute", "meta.decorator"],
  ["boolean", "constant.language.boolean"],
  ["comment", "comment"],
  ["comment.doc", "comment.block.documentation"],
  ["constant", "constant"],
  ["constructor", "entity.name.type.class"],
  ["emphasis", "markup.italic"],
  ["emphasis.strong", "markup.bold"],
  ["enum", "entity.name.type.enum"],
  ["function", "entity.name.function"],
  ["keyword", "keyword"],
  ["link_text", "string.other.link.title.markdown"],
  ["link_uri", "markup.underline.link.markdown"],
  ["number", "constant.numeric"],
  ["operator", "keyword.operator.arithmetic"],
  ["preproc", "keyword.control.directive"],
  ["property", "variable.other.property"],
  ["punctuation", "punctuation"],
  ["punctuation.bracket", "meta.brace.round"],
  ["punctuation.delimiter", "punctuation.separator.delimiter"],
  ["punctuation.list_marker", "punctuation.definition.list.begin.markdown"],
  ["punctuation.special", "punctuation.definition.template-expression.begin"],
  ["string", "string"],
  ["string.escape", "constant.character.escape"],
  ["string.regex", "string.regexp"],
  ["string.special", "string"],
  ["string.special.symbol", "constant.other.symbol"],
  ["tag", "entity.name.tag"],
  ["text.literal", "markup.inline.raw.markdown"],
  ["title", "markup.heading"],
  ["type", "entity.name.type"],
  ["variable", "variable"],
  ["variable.special", "variable.language"],
  ["variant", "variable.other.enummember"],
  // nvim-treesitter names (lumis) that differ from Zed's or need their own color.
  ["character", "string.quoted.single"],
  ["character.special", "constant.character.escape"],
  ["comment.documentation", "comment.block.documentation"],
  ["constant.builtin", "constant.language"],
  ["diff.delta", "markup.changed.diff"],
  ["diff.minus", "markup.deleted.diff"],
  ["diff.plus", "markup.inserted.diff"],
  ["function.builtin", "support.function"],
  ["keyword.operator", "keyword.operator.expression.typeof"],
  ["markup.heading", "markup.heading"],
  ["markup.italic", "markup.italic"],
  ["markup.link", "string.other.link.title.markdown"],
  ["markup.link.label", "string.other.link.description.markdown"],
  ["markup.link.url", "markup.underline.link.markdown"],
  ["markup.list", "punctuation.definition.list.begin.markdown"],
  ["markup.quote", "markup.quote.markdown"],
  ["markup.raw", "markup.inline.raw.markdown"],
  ["markup.strong", "markup.bold"],
  ["module", "entity.name.namespace"],
  ["namespace", "entity.name.namespace"],
  ["string.regexp", "string.regexp"],
  ["tag.attribute", "entity.other.attribute-name"],
  ["tag.delimiter", "punctuation.definition.tag"],
  ["type.builtin", "support.type.primitive"],
  ["variable.builtin", "variable.language"],
  ["variable.member", "variable.other.property"],
  ["variable.parameter", "variable.parameter"],
];

// Bold text: Pierre marks it with a color only; Zed themes use a weight.
const boldCaptures = new Set(["emphasis.strong", "markup.strong"]);

function fail(message: string): never {
  console.error(`port-pierre-theme: ${message}`);
  process.exit(2);
}

/** `#rgb`/`#rrggbb`/`#rrggbbaa` → lowercase `#rrggbbaa`. */
function normalize(color: string): string {
  const hex = color.toLowerCase().replace(/^#/, "");
  if (!/^[0-9a-f]+$/.test(hex)) fail(`not a hex color: ${color}`);
  if (hex.length === 3) return `#${[...hex].map((d) => d + d).join("")}ff`;
  if (hex.length === 6) return `#${hex}ff`;
  if (hex.length === 8) return `#${hex}`;
  return fail(`not a hex color: ${color}`);
}

function alphaOf(color: string): number {
  return parseInt(normalize(color).slice(7, 9), 16);
}

function withAlpha(color: string, alpha: number): string {
  const byte = Math.max(0, Math.min(255, Math.round(alpha)));
  return `${normalize(color).slice(0, 7)}${byte.toString(16).padStart(2, "0")}`;
}

/** Simple selectors of the rules, in order (descendant selectors skipped). */
function selectorsOf(theme: VsTheme): { selector: string; rule: TokenRule }[] {
  const out: { selector: string; rule: TokenRule }[] = [];
  for (const rule of theme.tokenColors) {
    const scopes =
      typeof rule.scope === "string" ? rule.scope.split(",") : rule.scope;
    for (const raw of scopes ?? []) {
      const selector = raw.trim();
      if (selector && !/[\s>]/.test(selector)) out.push({ selector, rule });
    }
  }
  return out;
}

/**
 * The value of `property` for TextMate scope `scope`: from the most specific
 * matching rule that sets it (a selector matches its own scope and every
 * dotted extension of it); among equally specific rules the later one wins.
 */
function resolveProperty({
  theme,
  scope,
  property,
}: {
  theme: VsTheme;
  scope: string;
  property: "foreground" | "fontStyle";
}): string | undefined {
  let best: { depth: number; value: string } | undefined;
  for (const { selector, rule } of selectorsOf(theme)) {
    const value = rule.settings[property];
    if (value === undefined) continue;
    if (scope !== selector && !scope.startsWith(`${selector}.`)) continue;
    const depth = selector.split(".").length;
    if (!best || depth >= best.depth) best = { depth, value };
  }
  return best?.value;
}

function syntaxOf(theme: VsTheme): Record<string, SyntaxEntry> {
  const entries = captures.map(([capture, scope]): [string, SyntaxEntry] => {
    const foreground = resolveProperty({
      theme,
      scope,
      property: "foreground",
    });
    if (!foreground) fail(`${theme.name}: no color for ${capture} (${scope})`);
    const entry: SyntaxEntry = { color: normalize(foreground) };
    const fontStyle = resolveProperty({ theme, scope, property: "fontStyle" });
    if (fontStyle?.split(/\s+/).includes("italic")) entry.font_style = "italic";
    if (fontStyle?.split(/\s+/).includes("bold") || boldCaptures.has(capture))
      entry.font_weight = 700;
    return [capture, entry];
  });
  const keys = entries.map(([k]) => k);
  if (new Set(keys).size !== keys.length) fail("duplicate capture in table");
  entries.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return Object.fromEntries(entries);
}

function styleOf(theme: VsTheme): Record<string, unknown> {
  const c = (key: string): string => {
    const value = theme.colors[key];
    if (value === undefined) fail(`${theme.name}: missing color ${key}`);
    return normalize(value);
  };
  const transparent = "#00000000";
  const fg = c("editor.foreground");
  const muted = c("sideBar.foreground");
  const subtle = c("editorLineNumber.foreground");
  const accent = c("focusBorder");
  const border = c("panel.border");
  const hover = c("list.hoverBackground");
  const selected = c("list.activeSelectionBackground");

  // Diff rows use Pierre's diff-editor backgrounds; word highlights are the same
  // hue at twice the alpha (**Provisional**, tuned by hand).
  const created = c("gitDecoration.addedResourceForeground");
  const deleted = c("gitDecoration.deletedResourceForeground");
  const modified = c("gitDecoration.modifiedResourceForeground");
  const createdLine = c("diffEditor.insertedTextBackground");
  const deletedLine = c("diffEditor.deletedTextBackground");
  const lineAlpha = alphaOf(createdLine);

  const status = (color: string, background?: string) => ({
    color,
    background: background ?? withAlpha(color, 0x1a),
    border: withAlpha(color, 0x4d),
  });
  const statuses: Record<string, ReturnType<typeof status>> = {
    conflict: status(c("gitDecoration.conflictingResourceForeground")),
    created: status(created, createdLine),
    deleted: status(deleted, deletedLine),
    error: status(c("notificationsErrorIcon.foreground")),
    hidden: status(c("titleBar.inactiveForeground")),
    hint: status(subtle),
    ignored: status(c("gitDecoration.ignoredResourceForeground")),
    info: status(c("notificationsInfoIcon.foreground")),
    modified: status(modified, withAlpha(modified, lineAlpha)),
    predictive: status(c("input.placeholderForeground")),
    renamed: status(modified),
    success: status(created),
    unreachable: status(subtle),
    warning: status(c("notificationsWarningIcon.foreground")),
  };

  const style: Record<string, unknown> = {
    background: c("sideBar.background"),
    border,
    "border.variant": c("widget.border"),
    "border.focused": accent,
    "border.selected": accent,
    "border.transparent": transparent,
    "border.disabled": c("input.border"),
    "elevated_surface.background": c("quickInput.background"),
    "surface.background": c("sideBar.background"),
    "element.background": c("input.background"),
    "element.hover": hover,
    "element.active": selected,
    "element.selected": selected,
    "element.disabled": c("input.background"),
    "drop_target.background": hover,
    "ghost_element.background": transparent,
    "ghost_element.hover": hover,
    "ghost_element.active": selected,
    "ghost_element.selected": selected,
    "ghost_element.disabled": transparent,
    text: fg,
    "text.muted": muted,
    "text.placeholder": c("input.placeholderForeground"),
    "text.disabled": c("titleBar.inactiveForeground"),
    "text.accent": c("textLink.foreground"),
    icon: fg,
    "icon.muted": muted,
    "icon.disabled": c("titleBar.inactiveForeground"),
    "icon.placeholder": c("input.placeholderForeground"),
    "icon.accent": accent,
    "status_bar.background": c("statusBar.background"),
    "title_bar.background": c("titleBar.activeBackground"),
    "title_bar.inactive_background": c("titleBar.inactiveBackground"),
    "toolbar.background": c("editor.background"),
    "tab_bar.background": c("editorGroupHeader.tabsBackground"),
    "tab.inactive_background": c("tab.inactiveBackground"),
    "tab.active_background": c("tab.activeBackground"),
    "search.match_background": c("editor.selectionBackground"),
    "panel.background": c("panel.background"),
    "panel.focused_border": accent,
    "pane.focused_border": accent,
    "scrollbar.thumb.background": withAlpha(subtle, 0x4d),
    "scrollbar.thumb.hover_background": withAlpha(subtle, 0x80),
    "scrollbar.thumb.border": transparent,
    "scrollbar.track.background": transparent,
    "scrollbar.track.border": border,
    "editor.foreground": fg,
    "editor.background": c("editor.background"),
    "editor.gutter.background": c("editor.background"),
    "editor.subheader.background": c("sideBarSectionHeader.background"),
    "editor.active_line.background": c("editor.lineHighlightBackground"),
    "editor.highlighted_line.background": c("editor.lineHighlightBackground"),
    "editor.line_number": subtle,
    "editor.active_line_number": c("editorLineNumber.activeForeground"),
    "editor.invisible": c("editorIndentGuide.activeBackground"),
    "editor.wrap_guide": c("editorIndentGuide.background"),
    "editor.active_wrap_guide": c("editorIndentGuide.activeBackground"),
    "editor.indent_guide": c("editorIndentGuide.background"),
    "editor.indent_guide_active": c("editorIndentGuide.activeBackground"),
    "editor.document_highlight.read_background": c(
      "editor.selectionBackground",
    ),
    "editor.document_highlight.write_background": c(
      "editor.selectionBackground",
    ),
    "terminal.background": c("terminal.background"),
    "terminal.foreground": c("terminal.foreground"),
    "terminal.bright_foreground": fg,
    "terminal.dim_foreground": subtle,
  };
  for (const name of [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
  ]) {
    const vs = name[0]!.toUpperCase() + name.slice(1);
    style[`terminal.ansi.${name}`] = c(`terminal.ansi${vs}`);
    style[`terminal.ansi.bright_${name}`] = c(`terminal.ansiBright${vs}`);
  }
  style["link_text.hover"] = c("textLink.activeForeground");
  for (const [name, s] of Object.entries(statuses)) {
    style[name] = s.color;
    style[`${name}.background`] = s.background;
    style[`${name}.border`] = s.border;
  }
  style["version_control.added"] = created;
  style["version_control.deleted"] = deleted;
  style["version_control.modified"] = modified;
  style["version_control.renamed"] = modified;
  style["version_control.conflict"] = statuses.conflict!.color;
  style["version_control.ignored"] = statuses.ignored!.color;
  style["version_control.word_added"] = withAlpha(
    createdLine,
    Math.min(255, lineAlpha * 2),
  );
  style["version_control.word_deleted"] = withAlpha(
    deletedLine,
    Math.min(255, alphaOf(deletedLine) * 2),
  );
  style.players = [
    {
      cursor: c("editorCursor.foreground"),
      background: c("editorCursor.foreground"),
      selection: c("editor.selectionBackground"),
    },
  ];
  style.syntax = syntaxOf(theme);
  return style;
}

function readPierre(variant: (typeof variants)[number]): {
  theme: VsTheme;
  version: string;
} {
  const packageJson = Bun.resolveSync("@pierre/theme/package.json", repoRoot);
  const version = (
    JSON.parse(readFileSync(packageJson, "utf8")) as { version: string }
  ).version;
  if (!version.startsWith("2."))
    fail(`expected @pierre/theme 2.x, found ${version}`);
  const path = Bun.resolveSync(
    `@pierre/theme/themes/pierre-${variant}.json`,
    repoRoot,
  );
  const theme = JSON.parse(readFileSync(path, "utf8")) as VsTheme;
  if (theme.type !== variant) fail(`${path}: type is ${theme.type}`);
  return { theme, version };
}

async function main(argv: string[]): Promise<void> {
  let outDir = join(repoRoot, "assets/themes");
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--out" && argv[i + 1]) outDir = resolve(argv[++i]!);
    else fail(`unknown argument: ${argv[i]}`);
  }
  mkdirSync(outDir, { recursive: true });
  for (const variant of variants) {
    const { theme, version } = readPierre(variant);
    const family = {
      $schema: "https://zed.dev/schema/themes/v0.2.0.json",
      name: theme.displayName,
      author: `Pierre Computer Company (@pierre/theme ${version}, Apache-2.0), ported by Polygloss`,
      themes: [
        {
          name: theme.displayName,
          appearance: variant,
          style: styleOf(theme),
        },
      ],
    };
    const outPath = join(outDir, `pierre-${variant}.json`);
    const options = (await resolveConfig(join(repoRoot, "package.json"))) ?? {};
    const text = await format(JSON.stringify(family, null, 2), {
      ...options,
      filepath: outPath,
    });
    writeFileSync(outPath, text);
    console.log(outPath);
  }
}

await main(process.argv.slice(2));
