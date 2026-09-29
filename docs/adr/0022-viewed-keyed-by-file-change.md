# ADR-0022: Viewed is keyed by file change

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§9 Viewed](../design.md#9-viewed)

## Context

GitHub's "Viewed" checkbox stays checked while a file is unchanged and clears itself when the file changes. With SHA-pinned diffs (ADR-0006), iterations (ADR-0007) and live states that are never pinned (ADR-0008), keying Viewed by `diff_id` would lose every mark on every edit.

## Decision

- Store Viewed per **file change**: `(path, old_blob, new_blob)`.
- It carries over while the change is unchanged, and unchecks itself when the file changes.
- It **never requires pinning**, because blob OIDs exist for unpinned states too.
- A "Changed since viewed" badge appears when the same review had viewed an earlier blob pair for that path.
- Marking a file viewed collapses it and jumps to the next unviewed file.

## Consequences

- Scope is global rather than per review (Provisional, OQ-8). Identical changes seen elsewhere count as viewed.
- A rebase that moves only the base changes `old_blob`, so the file becomes unviewed. Carrying Viewed over by patch-id is post-v1.
- Agents can read Viewed counts but cannot set them.

## Alternatives rejected

| Option                          | Why not                                        |
| ------------------------------- | ---------------------------------------------- |
| Per `diff_id`                   | Every iteration and live edit clears all marks |
| Per path only                   | Misses real changes                            |
| Blob pair plus `patch-id` now   | More complex; deferred                         |
| Sync with GitHub's viewed state | Needs the network (ADR-0005)                   |
