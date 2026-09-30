---
name: review-loop
description: Get your code changes reviewed by the human in Polygloss, their local GitHub-style diff review app, and act on their feedback. Use when the user asks you to open, show or send your work for review, to wait for or address their Polygloss review, or when a "Polygloss: the human submitted their review" reminder arrives.
---

# Polygloss review loop

Polygloss is the human's local code-review app: GitHub-style diffs, comment threads and Viewed checkboxes. The `polygloss` MCP server gives you its tools. This plugin also installs a Stop hook that wakes you when the human submits a review assigned to this session, so you never have to poll.

## 1. Open the diff

Call `open_diff` when your change is ready to look at. The default source is the live working tree against the merge-base with the default branch, which is what you want for uncommitted work. Use `source: { kind: "compare", base, head }` for a branch against a branch (a "PR"; pass a `label`) and `source: { kind: "commit", rev }` for one commit.

Keep the returned `review_id`. Opening assigns the review to this session, so the hook wakes you for it. `app: "unavailable"` is not an error: the review exists either way, and the human can open it later.

## 2. Optional guided tour

Use `create_comment` with `kind: "note"` to explain changes that are not obvious from the diff, anchored on the lines they describe (`anchor: { path, side: "new", line }`, `start_line` for a range, or `{ path }` for the whole file). Use `kind: "question"` only when you need the human to decide something. Stay well under the cap of about 50 per iteration, and keep each one short.

## 3. Hand over and end your turn

Tell the human the review is ready (the `url` from `open_diff` opens it) and end your turn. Do not call `wait_for_review` while this plugin is installed: the Stop hook waits for you, and a blocking call would hold the session. `wait_for_review(review_id)` is the fallback for clients without the hook, or when the human asks you to wait in the foreground.

## 4. When the human submits

You are woken with a reminder like `Polygloss: the human submitted their review of "…"`, which carries the verdict, the summary and the number of open threads. Then:

- `list_threads(review_id, status: "open")` for the open threads. Lists are paginated: pass `next_cursor` back as `cursor` until it is absent.
- `get_thread(thread_id)` for the full conversation, the anchored code and any suggestions.

Human comments become visible only when the human presses Submit review; you never see their drafts.

## 5. Fix, reply and resolve

Fix the code. Then `reply` to each thread saying what changed. Resolve a thread only when it is fully addressed (`reply` with `resolve: true`, or `resolve` with an optional closing `body_md`). When the answer is "no" or "not yet", reply and leave the thread open.

Apply ` ```suggestion ` blocks yourself: each replaces the anchored new-side lines with its contents.

You can `edit_comment` or `delete_comment` only your own comments. Use `focus` to scroll the app to a file, line or thread when you point the human at something; it never opens an editor.

## 6. Ask for a re-review

Call `request_rereview(review_id, summary_md)` with a short summary of what changed, then end your turn again. The live working tree becomes the next iteration and the human is notified.

The loop ends with the verdict: `approve` means done, so report that and stop. `request_changes` means keep going from step 4. `comment` means read the threads and use judgment.

## Line numbers

Lines are 1-based line numbers of the file on that side: `old` is the base, `new` is the head. Thread positions are reported against the latest iteration; a position of `outdated` or `absent` means the code moved or disappeared since the comment was written.

## If the tools are missing

If the `polygloss` tools do not appear, the plugin could not find the Polygloss CLI. Ask the human to install Polygloss.app and launch it once (it links its CLI into `~/Library/Application Support/polygloss/bin/`), or to put `polygloss` on `PATH`.
