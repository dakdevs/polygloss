# ADR-0024: Themes, fonts and syntax highlighting

- **Status:** Accepted. Supersedes the Zed-spike plan to drop lumis.
- **Date:** 2026-09-28
- **Design:** [§11.10 Themes and fonts](../design.md#1110-themes-and-fonts-adr-0024), [§11.11 Syntax highlighting](../design.md#1111-syntax-highlighting)

## Context

The target look is Pierre's diffs (diffs.com). Users of Zed-like editors already have themes they like. Syntax highlighting has to be fast, off the UI thread, and permissively licensed. Research measured lumis at about 5× faster than Shiki natively. It shares tree-sitter 0.26 with gpui-kit, whereas arborium's tree-sitter fork collides with it.

## Decision

- **Pierre Light / Pierre Dark** are the defaults: a port of the Apache-2.0 `@pierre/theme` 2.0 into Zed's theme JSON format. They follow the system appearance.
- **Any Zed theme JSON** can be loaded from `~/.config/polygloss/themes/`, parsed with our own model.
- Pierre's diff-style settings are supported: backgrounds, `+/-` indicators or bars, word diff, wrap.
- Code font: bundled **Lilex** (OFL), configurable. UI font: the system font.
- Highlighting uses **lumis 0.15** on the background executor with Budget and cancellation. lumis is the only tree-sitter user: every gpui-kit `tree-sitter*` feature stays off, because `tree-sitter`'s `links` key allows one version in the graph (library-choices §4).

## Consequences

- One palette drives the chrome (gpui-kit tokens), syntax captures (lumis) and diff colors.
- lumis colors will not match Shiki exactly, so the Pierre mapping is tuned by hand.
- The grammar set is trimmed to keep the binary small.
- `NOTICE` credits the Pierre theme and Lilex.

## Alternatives rejected

| Option                          | Why not                                                         |
| ------------------------------- | --------------------------------------------------------------- |
| giallo (Shiki-identical colors) | EUPL-1.2 license                                                |
| arborium                        | Its tree-sitter fork collides (`links`) with lumis and gpui-kit |
| syntect                         | TextMate grammars; slower and less accurate than tree-sitter    |
| Zed `language` / theme crates   | GPL (from the dropped Zed plan)                                 |
