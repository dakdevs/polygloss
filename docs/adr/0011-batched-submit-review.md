# ADR-0011: Batched Submit review

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§8.3 Drafts and Submit review](../design.md#83-drafts-and-submit-review-adr-0011)

## Context

An agent that sees each comment as soon as it is typed starts acting on half-finished reviews. Human reviewers want to read everything first and then send a coherent set, the way GitHub's pending reviews work.

## Decision

- Human comments, both new threads and replies, are **drafts** until **Submit review**. Submitting takes a summary and a verdict: Request changes, Comment or Approve.
- Agent replies are never drafts. The app shows a banner such as "claude-code replied to N threads".
- MCP exposes a blocking `wait_for_review`. **Approve** tells the agent it is done.
- ` ```suggestion ` blocks render as a mini-diff and are exposed through MCP as structured data. The agent applies them; there is no Apply button in v1.

## Consequences

- Agents never see half-written reviews. Waiters fire only on submissions.
- Drafts are stored durably in SQLite. Adding one pins a live state.
- Resolve and unresolve take effect immediately (Provisional, OQ-10).
- Polygloss never writes to the user's files.

## Alternatives rejected

| Option                              | Why not                                           |
| ----------------------------------- | ------------------------------------------------- |
| Agents see each comment immediately | The agent thrashes and humans lose control        |
| A "send" button on every comment    | Tedious, and still not a batch                    |
| An Apply-suggestion button          | Polygloss would write files (out of scope for v1) |
