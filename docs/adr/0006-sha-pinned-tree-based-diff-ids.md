# ADR-0006: SHA-pinned, tree-based diff ids

- **Status:** Accepted. This refines two earlier versions of the rule (see Context).
- **Date:** 2026-09-28
- **Design:** [§4.1 `diff_id`](../design.md#41-diff_id)

## Context

Comments and Viewed state need a stable key. The rule changed twice before landing here:

1. `hash(repo, old_sha, new_sha)`.
2. The repo component was dropped: git OIDs are globally unique, so any clone should share state.
3. **Tree** OIDs replaced commit OIDs, so that amend and reword keep state.

Research recommended SHA-256 over a versioned text input. BLAKE3 adds nothing for inputs this small.

## Decision

`diff_id = sha256("polygloss/diff/v1", objfmt, base_tree, head_tree)`

- Every input resolves to trees first:
  - commit → (first parent, commit)
  - three-dot compare → (merge-base, head)
  - direct compare → (X, Y)
  - live → (base, snapshot)
- A new push or edit is a new diff (ADR-0007 groups them into reviews).
- No repo component. Whitespace, algorithm and rename options are view settings and are not part of the id.
- Comments anchor to `(path, side, line)` of **immutable blobs**, so the anchor does not depend on how hunks are computed.
- The exact byte encoding is Provisional (design OQ-1) and is frozen at the first release.

## Consequences

- Any clone or worktree with both trees opens the same diff and sees its comments and Viewed marks.
- Amend, reword and no-op rebase keep all state.
- Unrelated repos with identical tree pairs (templates, forks) share threads. This is accepted.
- Moving refs need the review and iteration layer (ADR-0007).
- Golden test vectors cover sha1 and sha256 repos.

## Alternatives rejected

| Option                               | Why not                                                        |
| ------------------------------------ | -------------------------------------------------------------- |
| Commit OIDs                          | Amend and reword would lose comments                           |
| Intent-addressed ids (PR #N, branch) | Mutable, so the same id would mean different content over time |
| Including repo identity              | Breaks sharing across clones and worktrees                     |
| Including view options               | Splits one diff's comments across settings                     |
| Random UUIDs (diffle-style)          | Not deterministic                                              |
| BLAKE3                               | No benefit for inputs this small                               |
