# ADR-0023: One window with tabs, and the entry points

- **Status:** Accepted. The log says both "single binary" and "`polygloss mcp` = separate slim binary"; the Provisional reading is a GUI plus a slim CLI binary (ADR-0012, ADR-0019, design OQ-29).
- **Date:** 2026-09-28
- **Design:** [§11.1 Window and tabs](../design.md#111-window-and-tabs-adr-0023), [§13.4 Launch](../design.md#134-launch-and-single-instance), [§14 CLI](../design.md#14-cli)

## Context

Reviews start from a terminal (humans and agents), from a URL, or from inside the app. Several windows or instances would scatter state and make routing "open this diff" ambiguous.

## Decision

- The app runs as a **single instance** with **one window and tabs**.
- CLI entry points:
  - `polygloss`: live diff of the cwd, base = merge-base
  - `polygloss show <commit>`
  - `polygloss compare <base> <head>`: three-dot by default, `--direct` for two-dot
  - `polygloss open <diff_id>`
  - `polygloss mcp`
- A `polygloss://diff/<id>` URL scheme, registered in Info.plist and handled by GPUI `on_open_urls`.
- The in-app **⌘O** palette goes repo (recents or browse), then Live, a commit from the log, or a branch compare.
- **Home** lists recent reviews across all repos, with viewed and open-thread counts.
- The app keeps running after its last window closes, as macOS apps do.

## Consequences

- The CLI forwards requests over the app socket and launches the app when needed.
- In dev builds, single-instance behavior uses socket liveness plus a lock file.
- A tab corresponds to a review. Opening a review that is already open focuses its tab.

## Alternatives rejected

| Option              | Why not                                    |
| ------------------- | ------------------------------------------ |
| One window per diff | Window clutter; routing becomes ambiguous  |
| Several instances   | Duplicate watchers and conflicting state   |
| No URL scheme       | No deep links from agents or notifications |
