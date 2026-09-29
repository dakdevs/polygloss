# ADR-0009: Banner refresh, never auto-apply

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§10 Live mode](../design.md#10-live-mode), [§11.7 Banners](../design.md#117-banners)

## Context

Live diffs change while an agent edits files. If updates applied themselves, content would move under the reader and they would lose their place and line cursor. Research flagged live-refresh UX as an unresearched gap.

## Decision

- An FSEvents watcher (`notify`) watches the worktree and `.git/` (HEAD, index, refs). Changes trigger a debounced recompute in the background.
- The result is a **banner** at the top: "N files changed · Refresh (R)".
- Nothing changes until the user clicks the banner or presses `R`. The view never shifts under the user, including for the agent's own edits.
- Banners sit in a reserved strip or float as an overlay. They never insert rows into the viewport.
- Budget: filesystem event to banner in under 500 ms.

## Consequences

- The screen can show a stale state, and the banner says so.
- Refresh keeps the scroll anchor through line mapping, along with collapsed files, expanded context and Viewed marks.
- Unpinned states never create iterations.
- Compare reviews use the same banner when their refs move (Provisional, OQ-27).

## Alternatives rejected

| Option                          | Why not                                  |
| ------------------------------- | ---------------------------------------- |
| Auto-apply changes              | Moves content and loses the user's place |
| A new iteration on every change | Floods the review with iterations        |
| Timer-based polling             | Slower and more expensive than FSEvents  |
