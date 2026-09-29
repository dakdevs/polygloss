# ADR-0021: Open in editor

- **Status:** Accepted
- **Date:** 2026-09-28
- **Design:** [§11.13 Open in editor](../design.md#1113-open-in-editor-adr-0021)

## Context

Reviewers often jump from a diff line to the real file to check or fix something. The diff's line numbers belong to immutable blobs, but the file on disk may have changed since. Old-side lines and deleted files have no current file at all.

## Decision

- `o`, or the ⋯ menu in the file header, opens the **current on-disk file** at the **line-mapped** position: imara maps the diff's blob onto the on-disk content.
- Old-side lines and deleted files open a **read-only temp copy of the blob** instead.
- The editor is auto-detected (Zed, Cursor, VS Code, `$VISUAL`/`$EDITOR`), or set with a command template such as `zed {path}:{line}`.
- MCP `focus(diff_id, path, line)` scrolls Polygloss. Opening an editor is **human-only**.

## Consequences

- The editor is launched as an argv, never through a shell. Temp copies live under `~/Library/Caches/polygloss/blobs/` with mode `0444`.
- Terminal editors are Provisional (OQ-21).
- The detection code lives in the platform module.

## Alternatives rejected

| Option                             | Why not                               |
| ---------------------------------- | ------------------------------------- |
| Open at the raw diff line number   | Wrong as soon as the file has changed |
| Always open a blob copy            | Reviewers want to edit the real file  |
| Let agents open the human's editor | Surprising, and a security concern    |
