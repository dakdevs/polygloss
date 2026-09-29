# ADR-0015: One SQLite store (rusqlite, WAL)

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§7 Data model](../design.md#7-data-model-sqlite)

## Context

The app, the CLI, `polygloss mcp` and `polygloss wait` all read and write the same state. Research found:

- SQLite is the only engine checked that is safe with writers in several processes. Turso failed with lock errors; redb and fjall are single-process.
- Update hooks fire only on the connection that made the change, and `fs.watch` caught only 1 of 20 WAL commits.
- Every migrator checked races when several processes start at once.

## Decision

- One central database at `~/Library/Application Support/polygloss/polygloss.db`, shared by all processes.
- **rusqlite 0.40** with the `bundled` feature, in WAL mode.
- Bootstrap: `busy_timeout` first, then switch to WAL with a retry, then `synchronous=NORMAL` and `foreign_keys=ON`.
- Every write is `BEGIN IMMEDIATE`.
- **rusqlite_migration 2.6** runs under a file lock.
- Changes are detected by polling `PRAGMA data_version` and reading an append-only **events** table that each mutation writes in the same transaction.

## Consequences

- We build cross-process change notification ourselves. The same event cursors back MCP's `since` parameters and `polygloss wait`.
- The database must stay on a local disk; network and synced folders break WAL.
- Take a backup with `VACUUM INTO` before migrations, and run `quick_check` at startup.

## Alternatives rejected

| Option                  | Why not                                                |
| ----------------------- | ------------------------------------------------------ |
| One database per repo   | Breaks sharing across clones through `diff_id`         |
| redb, fjall (KV stores) | Single-process                                         |
| Turso                   | A second process failed with "file is locked"          |
| sqlx                    | Async-only, and its SQLite migration lock does nothing |
| Zed `sqlez`             | GPL                                                    |
| App as the only writer  | MCP tools would need the GUI running                   |
