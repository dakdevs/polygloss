# Polygloss for agents

Polygloss closes the review loop between a human and a coding agent, locally: the agent opens its changes in the human's review app, the human comments and presses **Submit review**, the agent wakes up, fixes the code, replies to each thread and asks for a re-review. Agents talk to Polygloss through a stdio MCP server (`polygloss mcp`) or the equivalent JSON CLI. Both work with the app closed; they share the app's SQLite store and only talk to the app to show things.

This page is for people wiring agents up and for agent authors. The app itself is described in the [user guide](user-guide.md).

## Contents

- [Claude Code plugin](#claude-code-plugin)
- [The review loop](#the-review-loop)
- [MCP server](#mcp-server)
- [JSON CLI](#json-cli)
- [Sessions and wake-up](#sessions-and-wake-up)
- [Other MCP clients](#other-mcp-clients)

## Claude Code plugin

The plugin is the recommended setup for Claude Code. It adds the `polygloss` MCP server, a Stop hook that wakes Claude when you submit a review, and a `review-loop` skill that teaches Claude the workflow.

Install Polygloss.app first and launch it once: it links its CLI into `~/Library/Application Support/polygloss/bin/polygloss`, where the plugin looks for it. Then, in a terminal:

```bash
claude plugin marketplace add dakdevs/polygloss
claude plugin install polygloss@polygloss
```

(or `/plugin marketplace add dakdevs/polygloss` and `/plugin install polygloss@polygloss` inside Claude Code; a local checkout works too: `claude plugin marketplace add /path/to/polygloss`). Restart Claude Code, then ask it to "open this in Polygloss for review".

The plugin's `bin/polygloss-shim` runs the first CLI it finds: `~/Library/Application Support/polygloss/bin/polygloss` (under `$POLYGLOSS_DATA_DIR` when set), then `polygloss` on `PATH`, then `/Applications/Polygloss.app/Contents/MacOS/polygloss-cli`. When there is none, it prints a one-line hint on stderr and exits 0, so an enabled plugin without Polygloss installed never fails a hook.

Without the plugin, register the server yourself (`claude mcp add --scope user polygloss -- polygloss mcp`). Everything works except the automatic wake-up: Claude then has to call `wait_for_review`.

## The review loop

1. **Open.** `open_diff` shows the agent's work. The default source is the live working tree against the merge-base with the default branch; `source.kind` `compare` reviews a branch against another (a "PR", with a `label`), and `commit` one commit. Keep the `review_id`. Opening assigns the review to the calling session.
2. **Tour (optional).** `create_comment` with `kind: "note"` explains non-obvious changes; `kind: "question"` asks the human to decide something. At most about 50 per iteration.
3. **Hand over.** Tell the human the review is ready and end the turn. With the plugin, the Stop hook waits; without it, call `wait_for_review(review_id)`.
4. **Read the feedback.** After a submission: `list_threads(review_id, status: "open")`, then `get_thread` for the conversation, the anchored code and any ` ```suggestion ` blocks. Human comments become visible only when the human submits; agents never see drafts.
5. **Fix, reply, resolve.** Fix the code, `reply` to each thread with what changed, and resolve only threads that are fully addressed (`reply` with `resolve: true`, or `resolve`). Apply suggestions by editing the files: a suggestion replaces the anchored new-side lines.
6. **Re-review.** `request_rereview(review_id, summary_md)` pins the working tree as the next iteration and notifies the human. Then wait again. The verdict ends the loop: `approve` means done, `request_changes` means keep going, `comment` means read the threads and use judgment.

Line numbers are 1-based lines of the file on one side: `old` is the base, `new` is the head. Thread positions are reported against the review's latest iteration (or the `diff_id` you pass) as `position.state`: `exact` (same lines), `moved` (same lines, new numbers in `position.line`), `outdated` (the lines changed since; compare `original_snippet` and `current_snippet`) or `absent` (the file or its side is not in that diff).

## MCP server

`polygloss mcp` is a stdio MCP server (it answers both the 2025 `initialize` handshake and the stateless 2026 protocol). It prints nothing but JSON-RPC on stdout and logs warnings to stderr. It never launches the app at startup; only `open_diff` (with `show`), `focus` and the `request_rereview` notification start it, in the background, when it is not running.

- **Author:** agent comments are signed with the client's `clientInfo.name` (for example `claude-code`).
- **Repository:** the `repo` parameter, else the first MCP root that is a git worktree (only `open_diff` asks the client for its roots), else `$CLAUDE_PROJECT_DIR`, else the server's working directory.
- **Results:** `structuredContent` plus the same JSON as text. Times are RFC 3339 UTC with milliseconds.
- **Paging:** every list stays under about 60,000 characters per page (Claude Code's default tool-output limit is 25k tokens). Pass `next_cursor` back as `cursor` until it is absent.
- **Errors:** `isError: true` with `{code, message}`. Codes: `not_found` (also a malformed or ambiguous id prefix), `repo_not_found`, `objects_missing`, `invalid_anchor`, `cap_exceeded`, `forbidden` (not your comment), `app_unavailable`, `conflict` (also a request the rules reject, such as an empty body) and `internal`.

### Tools

| Tool               | Parameters                                                                                                                                                 | Does                                                                                                                                                                                                        |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `open_diff`        | `repo?`, `source?` (`{kind: "live", since?}`, `{kind: "commit", rev}`, `{kind: "compare", base, head, mode?}`), `label?`, `show? = true`, `assign? = true` | Resolves the source, pins a live state, records the review and iteration, assigns it to you, and opens it in the app. Returns `review_id`, `diff_id`, `url`, stats and the first 200 files, with categories |
| `list_reviews`     | `repo?`, `status?`, `assigned? = "any" \| "me"`, `cursor?`, `limit? = 50`                                                                                  | Reviews with status, Viewed counts, open threads and questions, the last submission and any re-review request                                                                                               |
| `list_threads`     | `review_id` or `diff_id`, `status? = "open"`, `author?`, `kind?`, `path?`, `since?` (event seq), `cursor?`, `limit? = 50`                                  | Submitted threads with their current position and last comment, plus `latest_seq` to pass as `since` next time                                                                                              |
| `get_thread`       | `thread_id`, `cursor?`                                                                                                                                     | One thread: anchor, original and current snippets, the diff hunk, every comment with parsed suggestions, who resolved it. Long threads page their comments                                                  |
| `reply`            | `thread_id`, `body_md`, `resolve? = false`                                                                                                                 | Replies (published at once); with `resolve`, resolves in the same step                                                                                                                                      |
| `resolve`          | `thread_id`, `body_md?`                                                                                                                                    | Resolves a thread, with an optional closing reply                                                                                                                                                           |
| `unresolve`        | `thread_id`                                                                                                                                                | Reopens a resolved thread                                                                                                                                                                                   |
| `create_comment`   | `review_id` or `diff_id`, `kind: "note" \| "question"`, `body_md`, `anchor?` (`{path, side, line, start_line?}` or `{path}`; omit for the whole review)    | Starts a note or question thread (never a draft). Checks the anchor (`invalid_anchor`) and the per-iteration cap (`cap_exceeded`)                                                                           |
| `edit_comment`     | `comment_id`, `body_md`                                                                                                                                    | Edits one of your own comments                                                                                                                                                                              |
| `delete_comment`   | `comment_id`                                                                                                                                               | Deletes one of your own comments; one with replies leaves a placeholder                                                                                                                                     |
| `wait_for_review`  | `review_id`, `since?` (event seq; default now), `timeout_s? = 1500` (max 1500)                                                                             | Blocks until the human submits (or archives) the review; returns the verdict, summary and new or updated threads, or `timeout`. Sends progress every 60 s given a `progressToken`                           |
| `request_rereview` | `review_id`, `summary_md`                                                                                                                                  | Pins the working tree as a new iteration (live reviews), marks the review ready for re-review and notifies the human                                                                                        |
| `focus`            | `diff_id` or `review_id`, `path?`, `side?`, `line?`, `thread_id?`                                                                                          | Scrolls the app to a file, line or thread, opening the review if needed. Never opens an editor                                                                                                              |

`open_diff`, `list_threads` and `wait_for_review` carry `_meta["anthropic/alwaysLoad"]`, so Claude Code keeps them loaded when it defers other tools.

### Resources

Resource templates, also usable as `@`-mentions in Claude Code and as `polygloss://` links that open the app:

| URI                                      | Content (markdown)                                                |
| ---------------------------------------- | ----------------------------------------------------------------- |
| `polygloss://review/{review_id}`         | Status, iterations, last verdict and summary, counts              |
| `polygloss://review/{review_id}/threads` | Digest of the open threads: anchor, last comment, suggestion flag |
| `polygloss://thread/{thread_id}`         | The full thread                                                   |
| `polygloss://diff/{diff_id}`             | The file list with statuses, line counts and categories           |

`resources/list` returns the reviews assigned to your session plus the 20 most recent.

### File categories

A file's `category` says which supporting-file group the human's app moves it into, out of the main list: `tests`, `generated`, `vendored`, `agents`, `docs`, `tooling`, `stories`, or `custom:<id>`. Uncategorized files have none. Categories come from path patterns and the `linguist-generated` attribute, configured under `categories` in `settings.json` ([design §11.15](design.md#1115-file-categories-adr-0028) has the rules and built-in lists); by default only Tests and Generated are on. Every call reads the file again, so an edit applies to the next call; an invalid `categories` section counts as the defaults, with one warning on stderr.

- `stats.files`, `stats.additions` and `stats.deletions` count categorized files too, while the app's totals leave them out.
- `stats.categories` (`{"tests": {files, additions, deletions}, …}`, absent when nothing is categorized) counts every file of a category, but its lines only over the listed (first 200) files, like `stats`.
- The app's per-tab palette toggles are invisible to agents, so an agent's `category` can differ from what the human sees.
- `polygloss debug categorize [--repo <path>] [--json] <path>…` explains verdicts: the category, its title, the source (`built-in`, `extra`, `custom` or `attribute`), group and pattern, or the rescues that skipped one. The `attribute` source needs `--repo`, whose HEAD tree supplies `linguist-generated`. An invalid `categories` section is a `conflict` error here.

## JSON CLI

For agents without MCP, the `polygloss` CLI mirrors the tools with the same result shapes. Output is JSON when `--json` is given or stdout is not a terminal. Bodies come from a file, or stdin with `-`:

```bash
polygloss threads "$REVIEW_ID" --status open
printf 'Fixed: the loop now stops at len - 1.\n' | polygloss reply "$THREAD_ID" --body-file - --resolve
```

| Command                                                                                                                        | MCP tool           |
| ------------------------------------------------------------------------------------------------------------------------------ | ------------------ |
| `polygloss reviews [--status …] [--assigned me] [--cursor …] [--limit …]`                                                      | `list_reviews`     |
| `polygloss threads <review_id> [--status open\|resolved\|all] [--author …] [--kind …] [--path …] [--since <seq>] [--cursor …]` | `list_threads`     |
| `polygloss thread <thread_id> [--cursor …]`                                                                                    | `get_thread`       |
| `polygloss reply <thread_id> --body-file - [--resolve]`                                                                        | `reply`            |
| `polygloss resolve <thread_id> [--body-file -]`                                                                                | `resolve`          |
| `polygloss unresolve <thread_id>`                                                                                              | `unresolve`        |
| `polygloss edit <comment_id> --body-file -`                                                                                    | `edit_comment`     |
| `polygloss delete <comment_id>`                                                                                                | `delete_comment`   |
| `polygloss comment <review_id> --kind note\|question [--path … [--side old\|new] [--line … [--start-line …]]] --body-file -`   | `create_comment`   |
| `polygloss wait-review <review_id> [--since <seq>] [--timeout <s>]`                                                            | `wait_for_review`  |
| `polygloss rereview <review_id> --summary-file -`                                                                              | `request_rereview` |
| `polygloss focus <diff_id\|review_id> [--path … --side … --line …] [--thread <thread_id>]`                                     | `focus`            |

`open_diff` has human-facing twins: `polygloss [--since …] [<path>]` (live), `polygloss show <rev>`, `polygloss compare <base> <head> [--direct] [--label …]` and `polygloss open <diff_id>`; `polygloss snapshot [<path>]` pins the live state as a new iteration. Their reports count [categorized files](#file-categories) as `"categories": {"tests": 6, …}` (absent when none are), and their human output as "42 files (6 tests · 2 generated)".

Global flags, before or after the subcommand: `--repo <path>`, `--json`, `--no-open` (resolve and print ids without opening or launching the app), `--agent <name>` (the author name for writes; default `$POLYGLOSS_AGENT`, else `agent`) and `--session <id>`.

Errors exit 1, in JSON mode with `{"error": {"code", "message"}}` on stdout and the MCP error codes. Usage errors (unknown or missing arguments) exit 2 with the code `invalid_args`.

## Sessions and wake-up

Each agent session is identified by `CLAUDE_CODE_SESSION_ID` when Claude Code sets it, otherwise by a generated `pg-<uuid>` per `polygloss mcp` process (the JSON CLI takes `--session`). A review is **assigned** to one session: the one that last opened it with `open_diff` (pass `assign: false` to open without taking it). The human can reassign it from Home or the palette ("Assign to session…"). That session is the one woken on submit.

With the plugin, every Stop (the end of each Claude turn) runs, as an `asyncRewake` hook with a 3600 s timeout:

```text
polygloss wait --session "$CLAUDE_CODE_SESSION_ID"
```

`polygloss wait` exits at once (0) when no open review is assigned to the session. Otherwise it waits for a submission on one of the session's reviews and exits 2 with a summary on stderr (verdict, summary, the number of open threads, and "call list_threads(review_id=…)"), which wakes Claude even when the session is idle. It exits 0 about 30 s before its timeout (`--timeout`, default `$POLYGLOSS_WAIT_TIMEOUT_S`, else 3600) so the hook is never killed, and when a newer waiter for the same session replaces it. Submissions from before the review was assigned to the session never wake it.

Known limits:

- After the waiter times out, an idle session is not listening until its next turn. The Submit dialog tells the human ("claude-code isn't listening; it will see this on its next turn"), and they can nudge the agent.
- Claude Code enforces hook timeouts on `asyncRewake` hooks; whether it honors one as long as 3600 s is part of the release gate ([docs/testing/agent-wake-gate.md](testing/agent-wake-gate.md)).
- After `/clear`, `--continue` or `--resume`, the MCP server keeps its first session id while hooks get the new one. Polygloss links ids that share a Claude Code process, which covers the common cases.
- Opt-in instant push: `polygloss mcp --channel` (or `POLYGLOSS_MCP_CHANNEL=1`) also sends a `notifications/claude/channel` message on submit. It is a Claude Code research preview that needs `claude --dangerously-load-development-channels`; the plugin does not enable it.

## Other MCP clients

Any stdio MCP client can run `polygloss mcp`. Clients without Claude Code's hooks (Codex, Cursor, Claude Desktop and others) use `wait_for_review`, which blocks for up to 25 minutes per call; call it again with the returned `next_since` after a `timeout`. Shell-based agents can use `polygloss wait-review` the same way.

GUI apps do not see your shell's `PATH`, so give them an absolute path: `/Applications/Polygloss.app/Contents/MacOS/polygloss-cli`, or `/opt/homebrew/bin/polygloss` for a Homebrew install.

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.polygloss]
command = "/Applications/Polygloss.app/Contents/MacOS/polygloss-cli"
args = ["mcp"]
```

Cursor (`~/.cursor/mcp.json`), Claude Desktop (`~/Library/Application Support/Claude/claude_desktop_config.json`) and most other clients:

```json
{
  "mcpServers": {
    "polygloss": {
      "command": "/Applications/Polygloss.app/Contents/MacOS/polygloss-cli",
      "args": ["mcp"]
    }
  }
}
```

MCP comments are signed with the client's `clientInfo.name`; for the JSON CLI, set `--agent` or `POLYGLOSS_AGENT` to name the author. The server's instructions (sent on `initialize`) summarize the review loop for the model, so no extra prompt is needed.
