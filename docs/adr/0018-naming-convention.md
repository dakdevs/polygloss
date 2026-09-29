# ADR-0018: Naming convention

- **Status:** Accepted. The user approved an exception to the global kebab-case rule.
- **Date:** 2026-09-28
- **Design:** [§24 Repository layout](../design.md#24-repository-layout)

## Context

The user's global rule is kebab-case for every filename. Rust needs snake_case for module files, because `mod foo_bar;` looks for `foo_bar.rs`. The workaround, `#[path]` attributes, is fragile and not idiomatic.

## Decision

| Thing                                                             | Case                                                             |
| ----------------------------------------------------------------- | ---------------------------------------------------------------- |
| Crate directories and package names (`crates/polygloss-core`)     | kebab-case                                                       |
| TypeScript files, scripts, docs, fixtures, ADRs (`NNNN-title.md`) | kebab-case                                                       |
| Rust `.rs` module files                                           | snake_case (the compiler requires it; no `#[path]` hacks)        |
| Executables                                                       | `Polygloss` (GUI) and `polygloss-cli` (installed as `polygloss`) |

## Consequences

- Rust code looks idiomatic, and everything else follows the user's rule.
- The executable names differ by more than case, because the default APFS volume is case-insensitive.

## Alternatives rejected

| Option                                    | Why not                           |
| ----------------------------------------- | --------------------------------- |
| `#[path]` to force kebab-case `.rs` files | Fragile and surprising            |
| snake_case crate directories              | Breaks the user's kebab-case rule |
