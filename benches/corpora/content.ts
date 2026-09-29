// Code-like text for the perf corpora (plan T2.1, design §12.2): plausible
// source in 30 languages (so highlighting and shaping cost what real code
// costs), seeded edits with a target size, generated lockfiles and binary
// blobs. Only the shape matters; nothing here has to compile. Everything is
// integer or IEEE-exact arithmetic over the seeded PRNG, so output never
// depends on the JS engine's Math.log/exp.
import type { Rng } from "./lib";

const words = [
  "account",
  "active",
  "batch",
  "buffer",
  "cache",
  "config",
  "count",
  "cursor",
  "delta",
  "entry",
  "event",
  "field",
  "filter",
  "graph",
  "handle",
  "header",
  "index",
  "item",
  "key",
  "layout",
  "limit",
  "line",
  "merge",
  "node",
  "offset",
  "order",
  "parse",
  "path",
  "query",
  "range",
  "record",
  "render",
  "request",
  "result",
  "route",
  "scope",
  "session",
  "state",
  "status",
  "store",
  "stream",
  "task",
  "token",
  "total",
  "update",
  "user",
  "value",
  "window",
] as const;

export function word(rng: Rng): string {
  return rng.pick(words);
}

function cap(s: string): string {
  return `${s[0]?.toUpperCase() ?? ""}${s.slice(1)}`;
}

export function sentence(rng: Rng): string {
  const n = rng.int(5, 12);
  const ws = Array.from({ length: n }, () => word(rng));
  return `${cap(ws.join(" "))}.`;
}

/** A structured programming language: statement templates per construct. */
type Code = {
  kind: "code";
  /** Identifier style for `{n}`, `{m}` and `{a}`. */
  case: "snake" | "camel";
  indent: string;
  comment: string;
  header: string[];
  /** Opens a function; `\n` splits it into lines. Body goes one level in. */
  fn: string;
  /** Closing line for a function, conditional and loop ("" = none). */
  ends: [fn: string, cond: string, loop: string];
  decl: string;
  call: string;
  cond: string;
  loop: string;
  ret: string;
  /** A type declaration: opening line, field line (repeated), closing line. */
  type: [open: string, field: string, close: string];
};

type Family =
  | Code
  | {
      kind:
        | "markdown"
        | "html"
        | "xml"
        | "css"
        | "scss"
        | "json"
        | "yaml"
        | "toml"
        | "sql";
    };

/** One language of the corpora, identified by its file extension. */
export type Language = { ext: string; family: Family };

const brace = (
  c: Omit<Code, "kind" | "ends" | "comment" | "indent"> & {
    indent?: string;
  },
): Code => ({
  kind: "code",
  indent: "    ",
  comment: "//",
  ends: ["}", "}", "}"],
  ...c,
});

const code: Record<string, Code> = {
  rs: brace({
    case: "snake",
    header: ["use std::collections::HashMap;", "use crate::{m}::{T};"],
    fn: "pub fn {n}({a}: &str) -> Result<u32, Error> {",
    decl: "let {n} = {e};",
    call: "{n}.{m}({e})?;",
    cond: "if {c} {",
    loop: "for {v} in {n}.iter() {",
    ret: "Ok({e})",
    type: ["pub struct {T} {", "pub {n}: Vec<u32>,", "}"],
  }),
  ts: brace({
    case: "camel",
    indent: "  ",
    header: ['import { {T} } from "./{m}";'],
    fn: "export function {n}({a}: string): number {",
    decl: "const {n} = {e};",
    call: "{n}.{m}({e});",
    cond: "if ({c}) {",
    loop: "for (const {v} of {n}) {",
    ret: "return {e};",
    type: ["export type {T} = {", "{n}: number;", "};"],
  }),
  tsx: brace({
    case: "camel",
    indent: "  ",
    header: [
      'import { useMemo } from "react";',
      'import { {T} } from "./{m}";',
    ],
    fn: "export function {T}({ {a} }: {T}Props) {",
    decl: "const {n} = useMemo(() => {e}, [{a}]);",
    call: "{n}.{m}({e});",
    cond: "if ({c}) {",
    loop: "for (const {v} of {n}) {",
    ret: 'return <div className="{s}">{{e}}</div>;',
    type: ["type {T}Props = {", "{n}: string;", "};"],
  }),
  js: brace({
    case: "camel",
    indent: "  ",
    header: ['import { {m} } from "./{n}.js";'],
    fn: "function {n}({a}) {",
    decl: "let {n} = {e};",
    call: "{n}.{m}({e});",
    cond: "if ({c}) {",
    loop: "for (const {v} of {n}) {",
    ret: "return {e};",
    type: ["class {T} {", "{n} = null;", "}"],
  }),
  go: brace({
    case: "camel",
    indent: "\t",
    header: ["package {m}", "", 'import "fmt"'],
    fn: "func {n}({a} string) (int, error) {",
    decl: "{n} := {e}",
    call: "{n}.{M}({e})",
    cond: "if {c} {",
    loop: "for _, {v} := range {n} {",
    ret: "return {e}, nil",
    type: ["type {T} struct {", "{M} int", "}"],
  }),
  c: brace({
    case: "snake",
    header: ["#include <stdio.h>", '#include "{m}.h"'],
    fn: "static int {n}(const char *{a}) {",
    decl: "int {n} = {e};",
    call: "{m}({n}, {e});",
    cond: "if ({c}) {",
    loop: "for (size_t i = 0; i < {n}_len; i++) {",
    ret: "return {e};",
    type: ["struct {n} {", "int {m};", "};"],
  }),
  h: brace({
    case: "snake",
    header: ["#pragma once", "#include <stddef.h>"],
    fn: "static inline int {n}(const char *{a}) {",
    decl: "int {n} = {e};",
    call: "{m}({n}, {e});",
    cond: "if ({c}) {",
    loop: "for (size_t i = 0; i < {n}_len; i++) {",
    ret: "return {e};",
    type: ["typedef struct {T} {", "unsigned {n};", "} {T};"],
  }),
  cpp: brace({
    case: "snake",
    header: ["#include <string_view>", '#include "{m}.hpp"'],
    fn: "auto {n}(std::string_view {a}) -> int {",
    decl: "auto {n} = {e};",
    call: "{n}.{m}({e});",
    cond: "if ({c}) {",
    loop: "for (const auto& {v} : {n}) {",
    ret: "return {e};",
    type: ["class {T} {", "int {n}_ = 0;", "};"],
  }),
  java: brace({
    case: "camel",
    header: ["package com.example.{m};", "", "import java.util.List;"],
    fn: "public static int {n}(String {a}) {",
    decl: "var {n} = {e};",
    call: "{n}.{m}({e});",
    cond: "if ({c}) {",
    loop: "for (var {v} : {n}) {",
    ret: "return {e};",
    type: ["public record {T}(", "int {n},", ") {}"],
  }),
  kt: brace({
    case: "camel",
    header: ["package com.example.{m}", "", "import kotlin.math.max"],
    fn: "fun {n}({a}: String): Int {",
    decl: "val {n} = {e}",
    call: "{n}.{m}({e})",
    cond: "if ({c}) {",
    loop: "for ({v} in {n}) {",
    ret: "return {e}",
    type: ["data class {T}(", "val {n}: Int,", ")"],
  }),
  swift: brace({
    case: "camel",
    header: ["import Foundation"],
    fn: "func {n}(_ {a}: String) -> Int {",
    decl: "let {n} = {e}",
    call: "{n}.{m}({e})",
    cond: "if {c} {",
    loop: "for {v} in {n} {",
    ret: "return {e}",
    type: ["struct {T} {", "var {n}: Int", "}"],
  }),
  cs: brace({
    case: "camel",
    header: ["using System;", "using System.Linq;"],
    fn: "public static int {M}(string {a}) {",
    decl: "var {n} = {e};",
    call: "{n}.{M}({e});",
    cond: "if ({c}) {",
    loop: "foreach (var {v} in {n}) {",
    ret: "return {e};",
    type: ["public sealed class {T} {", "public int {M} { get; set; }", "}"],
  }),
  scala: brace({
    case: "camel",
    indent: "  ",
    header: ["package com.example.{m}", "", "import scala.util.Try"],
    fn: "def {n}({a}: String): Int = {",
    decl: "val {n} = {e}",
    call: "{n}.{m}({e})",
    cond: "if ({c}) {",
    loop: "for ({v} <- {n}) {",
    ret: "{e}",
    type: ["final case class {T}(", "{n}: Int,", ")"],
  }),
  php: brace({
    case: "camel",
    header: ["<?php", "", "declare(strict_types=1);"],
    fn: "function {n}(string ${a}): int {",
    decl: "${n} = {e};",
    call: "${n}->{m}({e});",
    cond: "if ({c}) {",
    loop: "foreach (${n} as ${v}) {",
    ret: "return {e};",
    type: ["final class {T} {", "private int ${n} = 0;", "}"],
  }),
  zig: brace({
    case: "camel",
    header: ['const std = @import("std");'],
    fn: "pub fn {n}({a}: []const u8) !u32 {",
    decl: "const {n} = {e};",
    call: "try {n}.{m}({e});",
    cond: "if ({c}) {",
    loop: "for ({n}) |{v}| {",
    ret: "return {e};",
    type: ["pub const {T} = struct {", "{n}: u32,", "};"],
  }),
  py: {
    kind: "code",
    case: "snake",
    indent: "    ",
    comment: "#",
    header: ['"""The {m} module."""', "", "import os"],
    fn: "def {n}({a}):",
    ends: ["", "", ""],
    decl: "{n} = {e}",
    call: "{n}.{m}({e})",
    cond: "if {c}:",
    loop: "for {v} in {n}:",
    ret: "return {e}",
    type: ["class {T}:", "{n}: int = 0", ""],
  },
  rb: {
    kind: "code",
    case: "snake",
    indent: "  ",
    comment: "#",
    header: ["# frozen_string_literal: true", "", 'require "{m}"'],
    fn: "def {n}({a})",
    ends: ["end", "end", "end"],
    decl: "{n} = {e}",
    call: "{n}.{m}({e})",
    cond: "if {c}",
    loop: "{n}.each do |{v}|",
    ret: "{e}",
    type: ["class {T}", "attr_reader :{n}", "end"],
  },
  lua: {
    kind: "code",
    case: "snake",
    indent: "  ",
    comment: "--",
    header: ['local {m} = require("{m}")'],
    fn: "local function {n}({a})",
    ends: ["end", "end", "end"],
    decl: "local {n} = {e}",
    call: "{n}:{m}({e})",
    cond: "if {c} then",
    loop: "for _, {v} in ipairs({n}) do",
    ret: "return {e}",
    type: ["local {T} = {", "{n} = 0,", "}"],
  },
  ex: {
    kind: "code",
    case: "snake",
    indent: "  ",
    comment: "#",
    header: ["defmodule {T} do", "  @moduledoc false"],
    fn: "def {n}({a}) do",
    ends: ["end", "end", "end"],
    decl: "{n} = {e}",
    call: "{T}.{m}({e})",
    cond: "if {c} do",
    loop: "for {v} <- {n} do",
    ret: "{e}",
    type: ["defmodule {T} do", "defstruct {n}: 0", "end"],
  },
  sh: {
    kind: "code",
    case: "snake",
    indent: "  ",
    comment: "#",
    header: ["#!/usr/bin/env bash", "set -euo pipefail"],
    fn: "{n}() {",
    ends: ["}", "fi", "done"],
    decl: 'local {n}="$({m} "${a}" {k})"',
    call: '{m} "${n}" {k}',
    cond: 'if [[ -n "${n}" ]]; then',
    loop: 'for {v} in "${{n}[@]}"; do',
    ret: 'echo "${n}"',
    type: ["readonly {U}={k}", "export {U}_{k}={k}", ""],
  },
  hs: {
    kind: "code",
    case: "camel",
    indent: "  ",
    comment: "--",
    header: ["module {T} where", "", "import Control.Monad (forM_, when)"],
    fn: "{n} :: String -> IO Int\n{n} {a} = do",
    ends: ["", "", ""],
    decl: "let {n} = {e}",
    call: "{n} <- {m} {a}",
    cond: "when ({c}) $ do",
    loop: "forM_ {n} $ \\{v} -> do",
    ret: "pure ({e})",
    type: ["data {T} = {T}", "{ {n} :: Int }", ""],
  },
};

const other = (
  kind: Exclude<Family, Code>["kind"],
): { kind: Exclude<Family, Code>["kind"] } => ({ kind });

/** The 30 languages of the synthetic corpus, by extension. */
export const languages: readonly Language[] = [
  ...Object.entries(code).map(([ext, family]) => ({ ext, family })),
  { ext: "md", family: other("markdown") },
  { ext: "html", family: other("html") },
  { ext: "xml", family: other("xml") },
  { ext: "css", family: other("css") },
  { ext: "scss", family: other("scss") },
  { ext: "json", family: other("json") },
  { ext: "yaml", family: other("yaml") },
  { ext: "toml", family: other("toml") },
  { ext: "sql", family: other("sql") },
];

export function language(ext: string): Language {
  const found = languages.find((l) => l.ext === ext);
  if (!found) throw new Error(`no corpus language for .${ext}`);
  return found;
}

// ---------------------------------------------------------------------------
// Templates

function ident(rng: Rng, style: "snake" | "camel"): string {
  const a = word(rng);
  const b = word(rng);
  return style === "snake" ? `${a}_${b}` : `${a}${cap(b)}`;
}

function expr(rng: Rng, style: "snake" | "camel"): string {
  switch (rng.int(0, 4)) {
    case 0:
      return `${ident(rng, style)}(${rng.int(0, 999)})`;
    case 1:
      return `${word(rng)}.${ident(rng, style)}(${ident(rng, style)}, ${rng.int(0, 99)})`;
    case 2:
      return `${ident(rng, style)} + ${rng.int(1, 64)}`;
    case 3:
      return `"${word(rng)} ${word(rng)} ${word(rng)}"`;
    default:
      return String(rng.int(0, 4096));
  }
}

/**
 * Expands `{x}` placeholders; every occurrence of one letter in a template
 * gets the same value. n/m/a identifiers, v loop variable, e expression,
 * c condition, s string text, k integer, T/M PascalCase, U UPPER_SNAKE.
 */
function expand(template: string, rng: Rng, style: "snake" | "camel"): string {
  const seen = new Map<string, string>();
  return template.replace(/\{([a-zA-Z])\}/g, (_, p: string) => {
    const known = seen.get(p);
    if (known !== undefined) return known;
    let v: string;
    switch (p) {
      case "n":
      case "m":
      case "a":
        v = ident(rng, style);
        break;
      case "v":
        v = word(rng);
        break;
      case "e":
        v = expr(rng, style);
        break;
      case "c":
        v = `${ident(rng, style)} > ${rng.int(0, 99)}`;
        break;
      case "s":
        v = `${word(rng)}-${word(rng)}`;
        break;
      case "k":
        v = String(rng.int(0, 999));
        break;
      case "U":
        v = `${word(rng)}_${word(rng)}`.toUpperCase();
        break;
      default:
        v = `${cap(word(rng))}${cap(word(rng))}`;
    }
    seen.set(p, v);
    return v;
  });
}

function statements(c: Code, rng: Rng, depth: number, out: string[]): void {
  const pad = c.indent.repeat(depth);
  const line = (t: string) => out.push(pad + expand(t, rng, c.case));
  const nested = (open: string, end: string) => {
    line(open);
    for (let i = rng.int(1, 3); i > 0; i--) statements(c, rng, depth + 1, out);
    if (end) out.push(pad + end);
  };
  const roll = depth >= 3 ? rng.int(0, 5) : rng.int(0, 9);
  if (roll <= 2) line(c.decl);
  else if (roll <= 4) line(c.call);
  else if (roll === 5) out.push("");
  else if (roll <= 7) nested(c.cond, c.ends[1]);
  else nested(c.loop, c.ends[2]);
}

function codeItem(c: Code, rng: Rng, out: string[]): void {
  out.push("");
  if (rng.chance(0.15)) {
    const [open, field, close] = c.type;
    out.push(expand(open, rng, c.case));
    for (let i = rng.int(2, 6); i > 0; i--)
      out.push(c.indent + expand(field, rng, c.case));
    if (close) out.push(expand(close, rng, c.case));
    return;
  }
  if (rng.chance(0.6)) out.push(`${c.comment} ${sentence(rng)}`);
  out.push(...expand(c.fn, rng, c.case).split("\n"));
  for (let i = rng.int(2, 6); i > 0; i--) statements(c, rng, 1, out);
  out.push(c.indent + expand(c.ret, rng, c.case));
  if (c.ends[0]) out.push(c.ends[0]);
}

/** One CSS declaration with random values (so stylesheets differ). */
function cssDeclaration(rng: Rng): string {
  const px = () => `${rng.int(0, 48)}px`;
  switch (rng.int(0, 7)) {
    case 0:
      return `padding: ${px()} ${px()}`;
    case 1:
      return `margin-${rng.pick(["top", "right", "bottom", "left"])}: ${px()}`;
    case 2:
      return `color: #${hex(rng, 6)}`;
    case 3:
      return `background: var(--${word(rng)}-${word(rng)})`;
    case 4:
      return `font-size: ${rng.int(10, 32)}px`;
    case 5:
      return `grid-template-columns: ${rng.int(1, 4)}fr ${px()} auto`;
    case 6:
      return `border: ${rng.int(1, 3)}px solid #${hex(rng, 6)}`;
    default:
      return `transition: ${word(rng)} ${rng.int(50, 400)}ms ease-out`;
  }
}

function otherItem(kind: string, rng: Rng, out: string[]): void {
  const n = () => ident(rng, "snake");
  switch (kind) {
    case "markdown": {
      const r = rng.int(0, 3);
      out.push("");
      if (r === 0) out.push(`## ${cap(word(rng))} ${word(rng)}`);
      else if (r === 1)
        for (let i = rng.int(1, 4); i > 0; i--) out.push(sentence(rng));
      else if (r === 2)
        for (let i = rng.int(2, 6); i > 0; i--) out.push(`- ${sentence(rng)}`);
      else {
        out.push("```rust");
        const rs = code.rs as Code;
        for (let i = rng.int(2, 5); i > 0; i--) statements(rs, rng, 0, out);
        out.push("```");
      }
      return;
    }
    case "html": {
      const tag = rng.pick(["section", "article", "aside", "div"]);
      out.push(`<${tag} class="${word(rng)}-${word(rng)}" id="${n()}">`);
      out.push(`  <h2>${cap(word(rng))} ${word(rng)}</h2>`);
      out.push(`  <p>${sentence(rng)}</p>`);
      out.push(`  <ul class="${word(rng)}-list">`);
      for (let i = rng.int(1, 5); i > 0; i--)
        out.push(
          `    <li><a href="/${word(rng)}/${rng.int(1, 99)}">${word(rng)}</a></li>`,
        );
      out.push("  </ul>", `</${tag}>`);
      return;
    }
    case "xml": {
      const tag = word(rng);
      out.push(`<${tag} id="${rng.int(1, 9999)}" name="${n()}">`);
      for (let i = rng.int(1, 5); i > 0; i--) {
        const t = word(rng);
        out.push(`  <${t}>${word(rng)} ${rng.int(0, 99)}</${t}>`);
      }
      out.push(`</${tag}>`);
      return;
    }
    case "css":
    case "scss":
      if (kind === "scss" && rng.chance(0.3))
        out.push(`$${word(rng)}-${word(rng)}: ${rng.int(1, 32)}px;`);
      out.push(`.${word(rng)}-${word(rng)} {`);
      for (let i = rng.int(2, 6); i > 0; i--)
        out.push(`  ${cssDeclaration(rng)};`);
      if (kind === "scss" && rng.chance(0.5)) {
        out.push(
          `  &:${rng.pick(["hover", "focus", "active"])} {`,
          `    ${cssDeclaration(rng)};`,
          "  }",
        );
      }
      out.push("}", "");
      return;
    case "json":
      out.push("  {");
      out.push(`    "id": ${rng.int(1, 99999)},`);
      out.push(`    "name": "${word(rng)} ${word(rng)}",`);
      out.push(`    "tags": ["${word(rng)}", "${word(rng)}"],`);
      out.push(`    "enabled": ${rng.chance(0.5)}`);
      out.push("  },");
      return;
    case "yaml":
      out.push(`${n()}:`);
      out.push(`  ${word(rng)}: ${rng.int(0, 999)}`);
      out.push(`  ${word(rng)}: "${word(rng)} ${word(rng)}"`);
      out.push(`  ${word(rng)}:`);
      for (let i = rng.int(1, 4); i > 0; i--) out.push(`    - ${n()}`);
      return;
    case "toml":
      out.push("", `[${word(rng)}.${word(rng)}]`);
      out.push(`${n()} = ${rng.int(0, 999)}`);
      out.push(`${n()} = "${word(rng)}"`);
      out.push(
        `${n()} = [${rng.int(0, 9)}, ${rng.int(0, 9)}, ${rng.int(0, 9)}]`,
      );
      return;
    default: {
      // sql
      const table = n();
      const r = rng.int(0, 3);
      if (r === 0) {
        out.push("", `CREATE TABLE ${table} (`, "  id INTEGER PRIMARY KEY,");
        for (let i = rng.int(1, 5); i > 0; i--)
          out.push(
            `  ${n()} ${rng.pick(["TEXT", "INTEGER", "REAL"])} NOT NULL,`,
          );
        out.push(`  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`, ");");
      } else if (r === 1) {
        out.push(
          `SELECT ${n()}, ${n()} FROM ${table} WHERE ${n()} = ${rng.int(0, 99)};`,
        );
      } else if (r === 2) {
        out.push(
          `INSERT INTO ${table} (${n()}, ${n()}) VALUES (${rng.int(0, 99)}, '${word(rng)}');`,
        );
      } else {
        out.push(`-- ${sentence(rng)}`);
      }
    }
  }
}

function generate(
  lang: Language,
  rng: Rng,
  n: number,
  header: boolean,
): string[] {
  const f = lang.family;
  const out: string[] = [];
  if (f.kind === "code") {
    if (header) for (const h of f.header) out.push(expand(h, rng, f.case));
    while (out.length < n) codeItem(f, rng, out);
  } else {
    if (header && f.kind === "markdown")
      out.push(`# ${cap(word(rng))} ${word(rng)}`);
    if (header && f.kind === "json") out.push("[");
    while (out.length < n) otherItem(f.kind, rng, out);
  }
  out.length = Math.min(out.length, n);
  return out;
}

/** A whole file: `n` lines of plausible `lang` source (a header, then items). */
export function sourceLines(lang: Language, rng: Rng, n: number): string[] {
  return generate(lang, rng, n, true);
}

/** `n` lines of items only, for inserting into an existing file. */
export function bodyLines(lang: Language, rng: Rng, n: number): string[] {
  return generate(lang, rng, n, false);
}

// ---------------------------------------------------------------------------
// Edits

/**
 * Applies `hunks` seeded edits to `base` (replace, insert or delete; spread
 * evenly, at least 8 unchanged lines apart so they stay separate hunks)
 * adding plus deleting `changed` lines in total. New lines come from `fresh`.
 */
export function editLines(opts: {
  base: string[];
  changed: number;
  hunks: number;
  rng: Rng;
  fresh: (n: number) => string[];
}): string[] {
  const { base, rng } = opts;
  const hunks = Math.max(
    1,
    Math.min(opts.hunks, Math.floor(base.length / 12), opts.changed),
  );
  const weights = Array.from({ length: hunks }, () => 0.5 + rng.next());
  const sum = weights.reduce((a, b) => a + b, 0);
  const segment = base.length / hunks;
  const out: string[] = [];
  let pos = 0;
  let left = opts.changed;
  for (let h = 0; h < hunks; h++) {
    const size =
      h === hunks - 1
        ? left
        : Math.min(
            left,
            Math.max(1, Math.round((opts.changed * (weights[h] ?? 1)) / sum)),
          );
    left -= size;
    const from = Math.floor(h * segment);
    const to = Math.floor((h + 1) * segment);
    // Keep 4 untouched lines at each end of the segment.
    const room = Math.max(0, to - from - 8);
    const roll = rng.next();
    const wantDel =
      roll < 0.3
        ? 0
        : roll < 0.5
          ? size
          : Math.round(size * (0.3 + 0.4 * rng.next()));
    const del = Math.min(wantDel, room);
    const ins = size - del;
    const start = Math.min(from + 4, base.length) + rng.int(0, room - del);
    out.push(...base.slice(pos, start));
    if (ins > 0) out.push(...opts.fresh(ins));
    pos = start + del;
  }
  out.push(...base.slice(pos));
  return out;
}

/**
 * Share of a modified file's planned changed lines that `git diff --numstat`
 * counts: in replace hunks git pairs up the blank and closing lines that the
 * deleted and inserted blocks have in common. Measured on the full synthetic
 * corpus (0.915); `modifiedPair` plans that much more so counts hit target.
 */
const countedShare = 0.915;

/**
 * A modified file with about `changed` lines added plus deleted (as numstat
 * counts them), in hunks of 10 to 40 changed lines amid unchanged context.
 */
export function modifiedPair(
  lang: Language,
  rng: Rng,
  changed: number,
): { base: string[]; head: string[] } {
  const planned = Math.round(changed / countedShare);
  const base = sourceLines(
    lang,
    rng,
    Math.round(planned * (0.8 + 1.6 * rng.next())) + 24,
  );
  const head = editLines({
    base,
    changed: planned,
    hunks: Math.max(1, Math.round(planned / rng.int(10, 40))),
    rng,
    fresh: (n) => bodyLines(lang, rng, n),
  });
  return { base, head };
}

/** A file of `lines` lines, lightly edited (~6%) so a rename stays a rename. */
export function renamedPair(
  lang: Language,
  rng: Rng,
  lines: number,
): { base: string[]; head: string[]; changed: number } {
  const base = sourceLines(lang, rng, lines);
  const changed = Math.max(1, Math.round(lines * 0.06));
  const head = editLines({
    base,
    changed,
    hunks: 2,
    rng,
    fresh: (n) => bodyLines(lang, rng, n),
  });
  return { base, head, changed };
}

/** Text of a file: lines joined with a trailing newline. */
export function text(lines: string[]): string {
  return lines.length === 0 ? "" : `${lines.join("\n")}\n`;
}

// ---------------------------------------------------------------------------
// Lockfiles and binaries

export const lockfileNames = [
  "Cargo.lock",
  "package-lock.json",
  "yarn.lock",
  "pnpm-lock.yaml",
  "go.sum",
] as const;
export type LockfileName = (typeof lockfileNames)[number];

function hex(rng: Rng, n: number): string {
  let s = "";
  for (let i = 0; i < n; i++) s += "0123456789abcdef"[rng.int(0, 15)];
  return s;
}

function b64(rng: Rng, n: number): string {
  const alphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let s = "";
  for (let i = 0; i < n; i++) s += alphabet[rng.int(0, 63)];
  return s;
}

function version(rng: Rng): string {
  return `${rng.int(0, 4)}.${rng.int(0, 40)}.${rng.int(0, 20)}`;
}

function lockEntry(kind: LockfileName, rng: Rng, out: string[]): void {
  const pkg = `${word(rng)}-${word(rng)}`;
  const v = version(rng);
  switch (kind) {
    case "Cargo.lock":
      out.push(
        "",
        "[[package]]",
        `name = "${pkg}"`,
        `version = "${v}"`,
        'source = "registry+https://github.com/rust-lang/crates.io-index"',
        `checksum = "${hex(rng, 64)}"`,
        "dependencies = [",
        ` "${word(rng)}-${word(rng)}",`,
        "]",
      );
      return;
    case "package-lock.json":
      out.push(
        `    "node_modules/${pkg}": {`,
        `      "version": "${v}",`,
        `      "resolved": "https://registry.npmjs.org/${pkg}/-/${pkg}-${v}.tgz",`,
        `      "integrity": "sha512-${b64(rng, 86)}==",`,
        '      "license": "MIT"',
        "    },",
      );
      return;
    case "yarn.lock":
      out.push(
        "",
        `"${pkg}@^${v}":`,
        `  version "${v}"`,
        `  resolved "https://registry.yarnpkg.com/${pkg}/-/${pkg}-${v}.tgz#${hex(rng, 40)}"`,
        `  integrity sha512-${b64(rng, 86)}==`,
      );
      return;
    case "pnpm-lock.yaml":
      out.push(
        "",
        `  /${pkg}@${v}:`,
        `    resolution: {integrity: sha512-${b64(rng, 86)}==}`,
        "    dev: false",
      );
      return;
    case "go.sum":
      out.push(
        `github.com/${word(rng)}/${pkg} v${v} h1:${b64(rng, 43)}=`,
        `github.com/${word(rng)}/${pkg} v${v}/go.mod h1:${b64(rng, 43)}=`,
      );
  }
}

/** `n` lines of a lockfile of the given kind (entries only without `header`). */
export function lockfileLines(
  kind: LockfileName,
  rng: Rng,
  n: number,
  header = true,
): string[] {
  const out: string[] = !header
    ? []
    : kind === "Cargo.lock"
      ? ["# This file is automatically @generated by Cargo.", "version = 4"]
      : kind === "package-lock.json"
        ? ["{", '  "lockfileVersion": 3,', '  "packages": {']
        : kind === "pnpm-lock.yaml"
          ? ["lockfileVersion: '9.0'", "", "packages:"]
          : [];
  while (out.length < n) lockEntry(kind, rng, out);
  out.length = Math.min(out.length, n);
  return out;
}

/** `size` bytes that git sees as binary: a PNG signature, then noise. */
export function binaryBlob(rng: Rng, size: number): Buffer {
  const b = Buffer.alloc(size);
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(b);
  for (let i = 8; i < size; i++) b[i] = rng.int(0, 255);
  b[8] = 0; // a NUL early, whatever the noise
  return b;
}

/**
 * Heavy-tailed positive weights (most small, a few up to ~30x the smallest):
 * `1 / (0.03 + u)`, exact arithmetic only.
 */
export function weights(rng: Rng, n: number): number[] {
  return Array.from({ length: n }, () => 1 / (0.03 + rng.next()));
}

/**
 * Splits `total` into integer shares proportional to `w` (each ≥ `min`) that
 * add up to `total` exactly whenever `total ≥ min * w.length`.
 */
export function shares(total: number, w: number[], min = 1): number[] {
  const sum = w.reduce((a, b) => a + b, 0);
  const out = w.map((x) => Math.max(min, Math.floor((total * x) / sum)));
  let diff = total - out.reduce((a, b) => a + b, 0);
  for (let i = 0; diff !== 0 && out.length > 0; i = (i + 1) % out.length) {
    const cur = out[i] ?? min;
    if (diff > 0) {
      out[i] = cur + 1;
      diff--;
    } else if (cur > min) {
      out[i] = cur - 1;
      diff++;
    } else if (out.every((x) => x <= min)) break;
  }
  return out;
}
