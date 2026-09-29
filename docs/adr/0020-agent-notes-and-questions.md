# ADR-0020: Agent notes and questions

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§8.4 Agent notes and questions](../design.md#84-agent-notes-and-questions-adr-0020)

## Context

Agents are more useful when they can explain their own changes, like a guided tour, and ask the human for decisions, not only answer threads. Without limits and visual separation, agent annotations would bury the human's review.

## Decision

- Agents may create threads with `create_comment`. Agent threads carry an agent badge and are **never drafts**.
- There are two kinds:
  - **note**: an explanation or walkthrough. Collapsed by default; a "Hide agent notes" toggle hides all of them.
  - **question**: counts toward "waiting on you" until the human replies (in a submitted review) or resolves it.
- **Cap:** about 50 agent threads per iteration. The exact number and scope are Provisional (OQ-13).
- The pattern the server instructions teach is `open_diff`, then annotate, then `wait_for_review`.

## Consequences

- Home and the Dock badge count open agent questions.
- `threads.kind` is `comment` for humans and `note` or `question` for agents, and the schema enforces this.
- Going over the cap returns `cap_exceeded`. Replies do not count toward the cap.

## Alternatives rejected

| Option                                | Why not                                      |
| ------------------------------------- | -------------------------------------------- |
| Agents may only reply                 | No guided tours, and no way to ask questions |
| Agent threads look like human threads | Noise; the human can't triage them           |
| No cap                                | Agents flooding a review                     |
