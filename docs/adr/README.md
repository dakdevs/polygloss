# Architecture decision records

Each ADR records one major decision in four parts: Context, Decision, Consequences and Alternatives rejected. [`../design.md`](../design.md) is the builder-facing source of truth; the ADRs explain why it looks the way it does.

## Conventions

- Files are named `NNNN-kebab-title.md`, numbered in order and never reused.
- Status is one of **Proposed**, **Accepted** or **Superseded by ADR-NNNN**. To change a decision, write a new ADR and mark the old one superseded.
- Details the design log leaves open are marked **Provisional** and tracked in [design §26 Open questions](../design.md#26-open-questions), not here.

## Index

| #    | Decision                                                                                                | Status   | In one line                                                                                      |
| ---- | ------------------------------------------------------------------------------------------------------- | -------- | ------------------------------------------------------------------------------------------------ |
| 0001 | [Native GPUI only](0001-native-gpui-only.md)                                                            | Accepted | Pure Rust on GPUI; no web views, Tauri, Electron or GPUiX                                        |
| 0002 | [gpui-kit foundation](0002-gpui-kit-foundation.md)                                                      | Accepted | gpui-kit 0.7 on gpui-pre `=0.3.7` for all chrome; weekly upgrade chores accepted                 |
| 0003 | [Own diff viewport from scratch](0003-own-diff-viewport-from-scratch.md)                                | Accepted | We write the viewport; no Zed fork, no pierre-native vendoring                                   |
| 0004 | [License MIT OR Apache-2.0](0004-license-mit-or-apache-2.md)                                            | Accepted | Dual permissive license; system git, not bundled; no GPL, AGPL or FSL code                       |
| 0005 | [Fully offline, local-only](0005-fully-offline-local-only.md)                                           | Accepted | No fetch, no forge APIs; a PR is a local branch compare with a label                             |
| 0006 | [SHA-pinned tree-based diff ids](0006-sha-pinned-tree-based-diff-ids.md)                                | Accepted | `sha256("polygloss/diff/v1", objfmt, base_tree, head_tree)`; no repo component                   |
| 0007 | [Reviews and iterations](0007-reviews-and-iterations.md)                                                | Accepted | A repo-scoped review groups pinned diffs as iterations                                           |
| 0008 | [Live worktree snapshots, merge-base default](0008-live-worktree-snapshots-merge-base-default.md)       | Accepted | Temp-index `write-tree` snapshots, pinned on demand; base = `merge-base(HEAD, origin/<default>)` |
| 0009 | [Banner refresh, never auto-apply](0009-banner-refresh-never-auto-apply.md)                             | Accepted | The watcher shows "N files changed"; the user refreshes; the view never shifts                   |
| 0010 | [Comment carry-forward via line mapping](0010-comment-carry-forward-via-line-mapping.md)                | Accepted | Threads move if their lines are unchanged, otherwise become outdated                             |
| 0011 | [Batched Submit review](0011-batched-submit-review.md)                                                  | Accepted | Human comments are drafts until Submit with a verdict; agent replies are never drafts            |
| 0012 | [MCP over stdio, shared SQLite and a unix socket](0012-mcp-stdio-over-shared-sqlite-and-unix-socket.md) | Accepted | rmcp stdio server over the shared store; the app launches lazily for UI tools only               |
| 0013 | [asyncRewake wake-up](0013-async-rewake-wake-up.md)                                                     | Accepted | A plugin hook runs `polygloss wait`; exit 2 wakes the idle session; still needs a release gate   |
| 0014 | [git CLI + gix + imara hunk engine](0014-git-cli-gix-imara-hunk-engine.md)                              | Accepted | git for structure and renames; gix for blobs; imara for hunks and word diff                      |
| 0015 | [SQLite store (rusqlite, WAL)](0015-sqlite-rusqlite-wal-store.md)                                       | Accepted | One central database; `data_version` polling plus an events table                                |
| 0016 | [macOS Apple Silicon v1](0016-macos-apple-silicon-v1.md)                                                | Accepted | arm64 only; portable seams; Linux next                                                           |
| 0017 | [Tests split between cargo and bun](0017-testing-split-cargo-and-bun.md)                                | Accepted | nextest, insta and GPUI tests; bun black-box MCP and CLI suites                                  |
| 0018 | [Naming convention](0018-naming-convention.md)                                                          | Accepted | kebab-case everywhere except snake_case `.rs` module files                                       |
| 0019 | [Distribution and signing](0019-distribution-and-signing.md)                                            | Accepted | Notarized DMG, team 5U7E4UQ5M3, `dev.dak.polygloss`, Sparkle, Homebrew tap, plugin marketplace   |
| 0020 | [Agent notes and questions](0020-agent-notes-and-questions.md)                                          | Accepted | Agent threads of kind note or question, never drafts, about 50 per iteration                     |
| 0021 | [Open in editor](0021-open-in-editor.md)                                                                | Accepted | `o` opens the on-disk file at the line-mapped position; blob copies for the old side             |
| 0022 | [Viewed keyed by file change](0022-viewed-keyed-by-file-change.md)                                      | Accepted | `(path, old_blob, new_blob)`; carries over; never needs pinning                                  |
| 0023 | [One window with tabs, and the entry points](0023-single-window-tabs-and-entry-points.md)               | Accepted | Single instance, tabs, CLI, `polygloss://` URLs, ⌘O palette, Home; tab row superseded by 0026    |
| 0024 | [Themes, fonts and highlighting](0024-themes-fonts-and-highlighting.md)                                 | Accepted | Zed theme JSON, Lilex, lumis; default themes superseded by 0027                                  |
| 0025 | [Keyboard-first, remappable keymap](0025-keyboard-first-remappable-keymap.md)                           | Accepted | GPUI actions with a hot-reloaded `keymap.json`                                                   |
| 0026 | [Inset titlebar and sidebar navigation](0026-inset-titlebar-and-sidebar-navigation.md)                  | Accepted | Traffic lights in the sidebar, no tab row; open reviews listed in the sidebar; ⌘1–⌘9             |
| 0027 | [Polygloss themes and the card layout](0027-polygloss-themes-and-card-layout.md)                        | Accepted | Polygloss Light/Dark by default; file cards on a canvas; bar indicators; Lilex kept              |
| 0028 | [File categories](0028-file-categories.md)                                                              | Accepted | geld-style path categories in core; collapsed sections at the bottom; store v2 attribute         |
| 0029 | [Product motion](0029-product-motion.md)                                                                | Accepted | Two short motions (threads panel, banner notices); Reduce Motion followed live                   |
