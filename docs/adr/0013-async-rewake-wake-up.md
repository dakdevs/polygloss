# ADR-0013: Wake agents with an asyncRewake hook

- **Status:** Accepted, pending a real Claude Code release gate
- **Date:** 2026-09-28
- **Design:** [§16 Claude Code plugin](../design.md#16-claude-code-plugin)

## Context

When the human submits a review, an idle Claude Code session has to wake up and act. Research looked at three push paths:

| Path                                   | State                                                                                                           |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `claude/channel` notifications         | Research preview. Needs `--dangerously-load-development-channels` on every launch.                              |
| Long-poll tool (`wait_for_review`)     | Claude Code moves it to the background after 2 min. Whether its completion wakes an idle session is unverified. |
| Plugin command hook with `asyncRewake` | GA. The docs say that exit code 2 "wakes Claude immediately even when the session is idle".                     |

## Decision

- Ship a **Claude Code plugin** (in this repo's marketplace) with an `asyncRewake` hook that runs `polygloss wait --session $CLAUDE_CODE_SESSION_ID`. The waiter exits 2 with a summary when a review assigned to that session is submitted.
- `wait_for_review` is the long-poll fallback for other clients.
- `claude/channel` is **opt-in** only.

## Consequences

Known risks. Wake-on-submit must be proven in real Claude Code before v1 ships:

- The hook `timeout` is enforced for asyncRewake (600 s by default). Whether larger values are honored has not been tested.
- An idle session cannot re-arm its hook, because Stop fires only on activity. After a timeout nobody is listening. The UI shows whether a waiter is live.
- The MCP subprocess keeps the session id it was spawned with. After `/clear` or `--continue`/`--resume`, that id can differ from the hook's. The Provisional fix links ids by owning process (design §16.4).
- Waiters are deduplicated per session. The plugin calls the CLI through a stable symlink path.

## Alternatives rejected

| Option                          | Why not                                                  |
| ------------------------------- | -------------------------------------------------------- |
| `claude/channel` as the default | Preview only, and needs a dangerous flag                 |
| Long-poll only                  | Idle-session delivery unverified; it ties up a tool call |
| The agent polls `list_threads`  | Wastes turns and misses idle periods                     |
