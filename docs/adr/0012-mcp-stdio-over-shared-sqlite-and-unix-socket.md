# ADR-0012: MCP over stdio, shared SQLite and a unix socket

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§13 Processes and IPC](../design.md#13-processes-and-ipc), [§15 MCP surface](../design.md#15-mcp-surface)

## Context

Research established three facts:

- Claude Code spawns stdio MCP servers at the start of every session, so launching the GUI eagerly is wrong.
- An HTTP endpoint inside the app needs tokens, lockfiles and Origin/Host checks.
- SQLite in WAL mode handles writers in several processes: 6,000 concurrent transactions with 0 `SQLITE_BUSY`.

The first research pick, an app that hosts HTTP MCP as the only writer, was demoted for these reasons.

## Decision

- `polygloss mcp` is a **stdio** server on **rmcp 3.5.x** (minor pinned). It lives in the slim CLI binary, which has no GPUI, and runs on a shared core crate over SQLite WAL, a store it shares with the app.
- Comment tools work with the GUI closed.
- Only UI tools (`open_diff`, `focus`, plus delivering re-review notifications) talk to the app. They connect over a **unix socket** in the `0700` App Support directory and launch the app lazily (`open -g -b` or the URL scheme). The app is **never** launched at MCP startup.
- The app watches the store with `PRAGMA data_version` plus an events table.
- A review is assigned to the session that opened it, and can be reassigned.
- A JSON CLI serves agents that do not speak MCP (the diffity pattern).

## Consequences

- Correctness across processes comes from `BEGIN IMMEDIATE`, an events table written in the same transaction, and a migration lock.
- Two executables: `Polygloss` (GUI) and `polygloss-cli` (installed as `polygloss`). The log also says "single binary"; this split is Provisional (design OQ-29).
- The socket protocol is small JSON Lines with no exec operations. There is no HTTP surface in v1.

## Alternatives rejected

| Option                                                     | Why not                                                           |
| ---------------------------------------------------------- | ----------------------------------------------------------------- |
| Streamable HTTP in the app, with lockfile and bearer token | The GUI must run, plus token plumbing and a DNS-rebinding surface |
| A stdio shim forwarding everything to the app              | Every tool needs the GUI running                                  |
| The app as the only DB writer                              | Same problem; SQLite WAL handles multiple writers safely          |
| A `polygloss mcp` mode inside the GUI binary               | Slow cold start, and it links GPUI                                |
