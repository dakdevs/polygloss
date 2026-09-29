# ADR-0007: Reviews group iterations

- **Status:** Accepted. The PR key form was superseded by ADR-0005.
- **Date:** 2026-09-28
- **Design:** [§4.2 Review keys](../design.md#42-review-keys), [§7.2 Schema](../design.md#72-schema-v1)

## Context

Diff ids are pinned by SHA (ADR-0006), so every push, rebase or worktree edit produces a new diff. Reviewers still need continuity: comments, Viewed state and a verdict that follow the change as it evolves, like GitHub PRs and Gerrit patchsets.

## Decision

- A **review** record groups successive pinned diffs, called **iterations**. Reviews belong to one repo.
- Review keys:
  - Branch compares: the typed ref pair, `compare:<base>...<head>` or `..` for direct. A display label such as "PR #123" is optional.
  - Live reviews: `worktree:<repo>@<branch>#since=<base>`.
- Single commits: the log says they get no review record. The Provisional default is a degenerate `commit:<oid>` review with exactly one iteration, so submit, wait and recents still work (design OQ-2).
- A new iteration is recorded when the resolved tree pair changes (compare/commit) or when a live state is pinned.
- Viewed is stored per file change and carries over (ADR-0022). Comments carry forward (ADR-0010).
- "Changes since last review" is the pinned diff from the head at the last submission to the current head.

## Consequences

- Submissions, assignments, waiters and Home recents all hang off reviews.
- The iteration picker and the "Changes since last review" toggle live in the toolbar.
- After a rebase, the interdiff shows base changes as noise. `range-diff` is post-v1 (OQ-9).
- If a clone moves, its reviews are orphaned, but threads survive through `diff_id`.

## Alternatives rejected

| Option                           | Why not                               |
| -------------------------------- | ------------------------------------- |
| One diff per review, no grouping | Loses continuity between pushes       |
| A mutable diff per PR            | Breaks deterministic ids              |
| `github.com/owner/repo#N` keys   | Needs the network (ADR-0005)          |
| Gerrit-style Change-Id trailers  | Requires rewriting the user's commits |
