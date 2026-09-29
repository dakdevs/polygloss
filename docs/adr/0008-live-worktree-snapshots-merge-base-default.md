# ADR-0008: Live worktree snapshots, merge-base by default

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§5 Snapshots](../design.md#5-snapshots), [§10 Live mode](../design.md#10-live-mode)

## Context

Coding agents usually leave their work uncommitted, so research called the working tree the main agent use case. Reviewing it needs deterministic ids (ADR-0006), and it must never disturb the user's index, HEAD or refs.

## Decision

- Uncommitted changes are a v1 source, and they are **live**: they update as files change, through the banner (ADR-0009).
- A snapshot uses a temp `GIT_INDEX_FILE`, then `git add -A`, then `git write-tree`, which gives a tree SHA. The temp index lives outside the worktree.
- Pinned states are kept alive by `refs/polygloss/snapshots/<id>`.
- Unpinned live states are hashed without being written into the repo. The Provisional mechanism is a scratch object directory with the repo as an alternate (design OQ-4).
- A live state is pinned when:
  - a comment is added
  - an agent asks for an id
  - the user runs Snapshot
  - the user submits a review
- The default base is **`merge-base(HEAD, origin/<default-branch>)`**: the branch plus uncommitted work, which survives agent commits. The toolbar picker also offers `HEAD` and a fixed commit. Each base is its own review, `worktree:<repo>@<branch>#since=…`.

## Consequences

- The user's index, HEAD and branches are never touched. Only `refs/polygloss/*` is written, and only when pinning.
- `git add -A` runs the user's clean filters (for example LFS). On large dirty worktrees this can be slow and has to be measured.
- Unpinned and pinned states give the same OIDs, so Viewed keys and ids agree.
- The scratch store has to be managed, and unreferenced snapshot refs have to be pruned.

## Alternatives rejected

| Option                                          | Why not                                               |
| ----------------------------------------------- | ----------------------------------------------------- |
| Porcelain `git diff` against the index/worktree | No stable content id, and untracked files are missing |
| `git stash create`                              | Leaves out untracked files and mixes in index state   |
| `HEAD` as the default base                      | The diff disappears whenever the agent commits        |
| Writing every live state into the repo          | Churns objects in the user's `.git`                   |
