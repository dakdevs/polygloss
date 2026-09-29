# ADR-0003: Own diff viewport, written from scratch

- **Status:** Accepted. Supersedes the Zed-fork plan from the feasibility spike.
- **Date:** 2026-09-28
- **Design:** [§11.6 Diff viewport](../design.md#116-diff-viewport-adr-0003), [§12.4 Virtualization](../design.md#124-virtualization-strategy)

## Context

No permissive, maintained GPUI diff viewer exists on a mainstream GPUI source.

- **pierre-native-view** (Apache-2.0) only paints and handles input. Layout and highlighting live in TypeScript. The repo is 3 commits old and sits on a different GPUI fork. It measures every slot on every frame: 17 ms per frame at 1,000 slots.
- **Zed's editor, multi_buffer and buffer_diff.** The spike showed they can do most of the job: split and unified views, word diff, inline blocks, and line mapping in 37 µs. The costs are what ruled them out:
  - They are GPL-3.0.
  - They pull in 676 packages, the binary is 108–146 MB, and cold builds take 2–5 min.
  - rusqlite is capped at 0.32.1.
  - Split view needs a hidden Workspace with side effects. It truncated the user's real `telemetry.log`.
  - Upstream moves fast: about 600 commits a month to rebase against.
  - Scale still failed: 4.3 GB RSS and 23 ms frames at 3,000 files in split view with syntax.

The user: "I want this to be written from scratch."

## Decision

We write the diff viewport (an estimated 5–10k lines of Rust) and all app code ourselves, as a GPUI element on gpui-kit. We do not fork or vendor Zed or pierre-native code. pierre-native-view, zeron/Comet `changes.rs` and `comments.rs`, lgtm/rgitui and Zed are **study-only** references.

## Consequences

- This is the largest from-scratch piece. It includes:
  - split alignment with variable-height blocks
  - selection and copy
  - logical scroll anchoring
  - incremental syntax
  - file-level windowing
- We fully control virtualization and memory, the binary stays small, and there are no native `links` conflicts.
- These features are ours to build: unified view with two line-number columns, word diff on every changed pair, the gutter "+", rename headers, and gap expanders. Zed lacked these too.

## Alternatives rejected

| Option                                            | Why not                                                                                         |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| Zed fork with a patch queue                       | GPL, heavy, needs a monthly rebase, still failed scale. The user wants it written from scratch. |
| Vendor pierre-native-view                         | A JSON viewport with string-typed APIs, O(files) frame cost, and the wrong GPUI fork            |
| Port zeron/Comet (MIT)                            | On another GPUI fork. Kept as a design reference only.                                          |
| GitComet (AGPL), reviu (FSL), smolcars/hunk (GPL) | Licenses                                                                                        |
