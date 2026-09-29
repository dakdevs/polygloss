# ADR-0010: Comment carry-forward via line mapping

- **Status:** Accepted. Amends the earlier rule "no re-anchoring in v1".
- **Date:** 2026-09-28
- **Design:** [§8.6 Carry-forward](../design.md#86-carry-forward-and-outdated-adr-0010)

## Context

The first rule marked every comment from a prior iteration as outdated. In live mode that is far too noisy, since the diff changes on every save. GitHub marks a comment outdated on any change to its line. Gerrit carries comments across patchsets. Line mapping is cheap: 37 µs in the Zed spike, and imara is fast.

## Decision

- Map a comment's lines through a diff from the **comment's blob to the current blob**.
- If every commented line is unchanged, **move** the thread to the new line numbers.
- If any commented line changed, mark the thread **outdated** and show its original snippet.
- This applies to live diffs and to iterations of compare and commit reviews.
- Renames inside the current diff are followed.
- Outdated threads stay open, still take replies, and are also listed in the threads panel.
- Results are cached per `(thread, diff)` and carry an engine version.

## Consequences

- Comments survive edits to unrelated lines.
- The result is deterministic for a given engine version, and it is recomputed when the mapper changes.
- MCP returns both the original anchor and `position.state`.
- There is no fuzzy relocation in v1.

## Alternatives rejected

| Option                                                | Why not                                      |
| ----------------------------------------------------- | -------------------------------------------- |
| Every comment from a prior iteration is outdated      | Too noisy for live mode                      |
| Fuzzy or quote-based relocation (Phabricator ghosts)  | Not deterministic; possible post-v1          |
| Anchoring to positions inside the diff (hunk offsets) | Breaks when the algorithm or context changes |
