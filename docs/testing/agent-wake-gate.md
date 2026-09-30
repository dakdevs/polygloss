# Agent wake-up gate (manual)

This procedure proves the agent wake-up path end to end in real Claude Code: an idle Claude Code session wakes when you press **Submit review** in Polygloss (design §16, ADR-0013). It cannot be automated, because it needs a real, idle Claude Code session and a human pressing Submit. It is a **release gate**: W1–W3 gate milestone M4, and W1–W8 gate M5 and v1 ([plan: Manual gate](../plan.md#manual-gate-agent-wake-up-in-real-claude-code)).

**You** run the cases and record the results in the [Results](#results) table. Coding agents prepare the kit and never mark a case passed.

`tests/scripts/wake-gate.test.ts` checks the kit itself: `prepare.sh` in a sandbox, a W1 rehearsal without Claude (the plugin's Stop hook command wakes on a simulated Submit), and the plugin install and cleanup commands in a sandboxed Claude Code config.

## What the kit touches

| Where                                                    | W1–W7                                                                                                                                                                                                                                      |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `/tmp/polygloss-wake-gate/`                              | Everything `prepare.sh` creates: `data/` (the sandbox data dir: database, socket, stable CLI link `data/bin/polygloss`), `repo-a/` and `repo-b/` (scratch repos). `prepare.sh` replaces it on every run.                                   |
| `~/Library/Application Support/polygloss`                | Untouched: `POLYGLOSS_DATA_DIR` points the CLI, the MCP server, the hook and the dev app at the sandbox.                                                                                                                                   |
| `~/Library/Caches/polygloss`, `~/Library/Logs/polygloss` | Used as usual: `POLYGLOSS_DATA_DIR` moves only the data dir, so snapshot scratch objects and the app log land in the real cache and log dirs. The app log is where to look when a case fails.                                              |
| `~/.config/polygloss`                                    | Read as usual (settings, keymap, themes).                                                                                                                                                                                                  |
| Claude Code                                              | The plugin is installed at **local scope** in the scratch repos only (`<repo>/.claude/settings.local.json`, excluded from git so it never appears in the review). Other Claude Code sessions get no Polygloss MCP server and no Stop hook. |

`prepare.sh` never runs `claude` and never writes to your Claude Code config; you run the `claude plugin …` commands yourself.

## Setup

Run these before **every** case, so each case starts from a fresh database and fresh scratch repos. Quit Claude Code sessions and Polygloss from the previous case first: `prepare.sh` refuses to replace the gate root while any process still uses it.

1. From the Polygloss checkout, preview the steps, then prepare:

   ```bash
   scripts/wake-gate/prepare.sh --dry-run
   scripts/wake-gate/prepare.sh
   ```

   It builds `Polygloss` and `polygloss-cli` (`scripts/cargo.sh build -p …`, one package at a time), creates `/tmp/polygloss-wake-gate/data` (`0700`), links `data/bin/polygloss` to `<checkout>/target/debug/polygloss-cli` (the path the plugin's shim tries first), creates `repo-a` and `repo-b` (one commit, one uncommitted change to `src/scale.js` each), and prints the commands below with your paths filled in. `--no-build` reuses the binaries already built (for example between cases); `--root <dir>` picks another gate root.

2. Record the versions in the [Results](#results) table (the plan was written against Claude Code 2.1.283 on 2026-09-28):

   ```bash
   claude --version
   sw_vers -productVersion
   ```

3. In a **fresh terminal** (the env is inherited by the MCP server, the Stop hook and the dev app the MCP server launches). `<checkout>` is the Polygloss checkout `prepare.sh` ran from:

   ```bash
   export POLYGLOSS_DATA_DIR=/tmp/polygloss-wake-gate/data
   export POLYGLOSS_TEST=1
   export POLYGLOSS_APP_BIN=<checkout>/target/debug/Polygloss
   export PATH="$POLYGLOSS_DATA_DIR/bin:$PATH"
   cd /tmp/polygloss-wake-gate/repo-a
   claude plugin marketplace add <checkout> --scope local
   claude plugin install polygloss@polygloss --scope local
   claude
   ```

   If Claude Code asks whether you trust the folder, accept.

   **About `POLYGLOSS_TEST=1`.** Always export `POLYGLOSS_TEST=1` and `POLYGLOSS_APP_BIN` together: either one alone makes the launcher refuse to start the app (it never falls back to an installed bundle on the real data dir). `POLYGLOSS_TEST=1` also turns on the other test-only surfaces (plan OQ-P4) for everything started from this terminal, including Claude Code's MCP server and hook: the `debug_state` socket op and the hidden `polygloss debug …` commands that simulate a human. That is acceptable for W1–W7, which use the sandbox data dir and scratch repos. W8 runs without it (and without `POLYGLOSS_APP_BIN` and `POLYGLOSS_DATA_DIR`).

   The `PATH` line makes `polygloss` in your shell (and in Claude's Bash tool) the CLI you just built.

4. **Checks** used by the cases, from a second terminal tab with the same three exports and the `PATH` line:

   ```bash
   polygloss reviews --json
   pgrep -fl 'polygloss(-cli)? wait'
   sqlite3 "$POLYGLOSS_DATA_DIR/polygloss.db" 'select session_id, pid from waiters'
   sqlite3 "$POLYGLOSS_DATA_DIR/polygloss.db" 'select id, canonical_id, client_name, owner_pid from sessions'
   ```

   `polygloss reviews --json` lists each review with its `assigned_session`. A live waiter shows up in both `pgrep` (as `…/data/bin/polygloss wait --session <id>`) and the `waiters` table.

**Measuring wake latency:** note the time you click **Submit** (for example run `date +%T` in a spare terminal at that moment) and the time Claude's first new output appears.

## Cases

The prompt to type into Claude is the same in most cases:

```text
Rename x to y in src/scale.js, open it for review in Polygloss, then end your turn.
```

### W1 Idle wake

```bash
scripts/wake-gate/prepare.sh
# fresh terminal: the Setup step 3 block (repo-a), then in Claude type the prompt above
# second terminal, after Claude's turn has ended:
polygloss reviews --json
pgrep -fl 'polygloss(-cli)? wait'
```

1. Type the prompt. Claude edits `src/scale.js` and calls `open_diff`; Polygloss opens the review (possibly in the background).
2. When Claude's turn has ended, run the checks: the review's `assigned_session` is Claude's session (the `claude-code` row in `sessions`) and there is exactly **one** waiter.
3. Do not type anything into Claude. Wait **2 minutes**.
4. In Polygloss, add **2** line comments on `src/scale.js` and **Submit review** with **Request changes**. Note the time.

**Pass when:** within 10 s, with no typing, Claude wakes, calls `list_threads`/`get_thread`, fixes the code and replies to both threads. Record the wake latency.

### W2 Idle longer than 600 s

```bash
scripts/wake-gate/prepare.sh --no-build
# fresh terminal: the Setup step 3 block (repo-a), then in Claude type the prompt above
```

1. As W1 steps 1–2.
2. Wait **15 minutes** without typing into Claude, then run the checks again: the waiter is still there.
3. Add a comment and **Submit review** with **Request changes**. Note the time.

**Pass when:** Claude wakes. This proves the hook's `timeout: 3600` is honored for `asyncRewake` (the default is 600 s). Record the wake latency.

### W3 Waiter expired

```bash
scripts/wake-gate/prepare.sh --no-build
# fresh terminal: the Setup step 3 block (repo-a), but before `claude` also run:
export POLYGLOSS_WAIT_TIMEOUT_S=120
# then in Claude type the prompt above
```

1. Type the prompt. When Claude's turn has ended, the checks show one waiter.
2. Wait **3 minutes** (the waiter stops listening 30 s before its 120 s timeout). The checks now show **no** waiter.
3. Add a comment and open the **Submit review** dialog. Read the hint, then submit with **Request changes**.
4. Type a new message to Claude, for example `Anything new on the review?`.

**Pass when:** the dialog shows "claude-code isn't listening; it will see this on its next turn"; the transcript shows no hook error; on your next message Claude reads the submission through its tools (`list_reviews`, `list_threads`).

### W4 After `/clear`

```bash
scripts/wake-gate/prepare.sh --no-build
# fresh terminal: the Setup step 3 block (repo-a), then in Claude type the prompt above
# after Claude's turn has ended, in Claude:
/clear
# then ask something trivial, for example: What is 2 + 2?
# second terminal, after that turn has ended:
sqlite3 "$POLYGLOSS_DATA_DIR/polygloss.db" 'select id, canonical_id, client_name, owner_pid from sessions'
pgrep -fl 'polygloss(-cli)? wait'
```

1. Type the prompt and let Claude call `open_diff` and end its turn.
2. Run `/clear`, ask the trivial question and let that turn end.
3. Run the checks: the `sessions` table now has the old id (the MCP server's) and the new one, linked through `canonical_id` (same `owner_pid`); one waiter.
4. Add a comment and **Submit review** with **Request changes**.

**Pass when:** the new (post-`/clear`) session wakes (owner-pid linking, design §16.4). Record both session ids from the `sessions` table in the notes.

### W5 After `--resume`

```bash
scripts/wake-gate/prepare.sh --no-build
# fresh terminal: the Setup step 3 block (repo-a), then in Claude type the prompt above
# after Claude's turn has ended, note the review's assigned_session:
polygloss reviews --json
# quit Claude (/exit), then in the same terminal:
claude --resume <session id>
# ask something trivial so a turn ends, for example: What is 2 + 2?
```

1. Type the prompt and let Claude call `open_diff` and end its turn. Note the session id (`assigned_session`).
2. Quit Claude with `/exit`, resume it with `claude --resume <session id>`, ask the trivial question and let that turn end. The checks show one waiter.
3. Add a comment and **Submit review** with **Request changes**.

**Pass when:** Claude wakes in the resumed session.

### W6 Approve ends the loop

```bash
scripts/wake-gate/prepare.sh --no-build
# fresh terminal: the Setup step 3 block (repo-a), then in Claude type the prompt above
# second terminal, after the approval has been handled:
pgrep -fl 'polygloss(-cli)? wait'
polygloss reviews --json
```

1. Run W1 through Claude's replies. Claude then calls `request_rereview` (the plugin's `review-loop` skill tells it to; if it does not, type `Request a re-review.`). Polygloss shows the re-review banner.
2. **Submit review** with **Approve**.
3. After Claude's turn ends, run the checks.

**Pass when:** Claude wakes, reports that the review is done and does not wait again: no waiter is left and the review's `status` is no longer `open`.

### W7 Two sessions

```bash
scripts/wake-gate/prepare.sh --no-build
# terminal A: the Setup step 3 block (repo-a), then in Claude type the prompt above
# terminal B: the same exports and PATH line, then:
cd /tmp/polygloss-wake-gate/repo-b
claude plugin marketplace add <checkout> --scope local
claude plugin install polygloss@polygloss --scope local
claude
# in Claude B type the same prompt; after both turns have ended:
polygloss reviews --json
pgrep -fl 'polygloss(-cli)? wait'
```

1. Start both sessions and give each the prompt. The checks show two reviews, each assigned to its own session, and two waiters.
2. Submit **only review A** (the `repo-a` tab) with **Request changes**.

**Pass when:** only session A wakes. Session B stays idle and its waiter is still listed.

### W8 App closed, bundled

This case is part of the M5 gate: it needs the packaged app (T5.1) installed in `/Applications`. It uses the **real** data dir and runs **without** `POLYGLOSS_TEST=1`, `POLYGLOSS_APP_BIN` and `POLYGLOSS_DATA_DIR`.

```bash
scripts/wake-gate/prepare.sh --no-build
# fresh terminal, none of the Setup exports:
unset POLYGLOSS_DATA_DIR POLYGLOSS_TEST POLYGLOSS_APP_BIN POLYGLOSS_WAIT_TIMEOUT_S
open -a Polygloss
# quit Polygloss (⌘Q) once its window is up: the launch refreshed
# ~/Library/Application Support/polygloss/bin/polygloss, which the shim now finds
cd /tmp/polygloss-wake-gate/repo-a
claude plugin marketplace add <checkout> --scope local
claude plugin install polygloss@polygloss --scope local
claude
```

1. Type the prompt. Add a comment and **Submit review** with **Request changes**; Claude wakes and starts fixing.
2. While Claude works, quit Polygloss (⌘Q) and bring another app to the front.
3. Let Claude reply and call `request_rereview`.

**Pass when:** Polygloss launches hidden, a macOS notification appears, the Dock badge shows 1, and clicking the notification focuses the review tab.

## Results

Fill in one row per case run (add rows for reruns). Result is pass or fail; wake latency is Submit click → first Claude output.

| Case | Date | Claude Code version | macOS version | Result | Wake latency | Notes |
| ---- | ---- | ------------------- | ------------- | ------ | ------------ | ----- |
| W1   |      |                     |               |        |              |       |
| W2   |      |                     |               |        |              |       |
| W3   |      |                     |               |        |              |       |
| W4   |      |                     |               |        |              |       |
| W5   |      |                     |               |        |              |       |
| W6   |      |                     |               |        |              |       |
| W7   |      |                     |               |        |              |       |
| W8   |      |                     |               |        |              |       |

## If a case fails

Record the failure in the Results table first; you decide whether v1 ships with the fallback.

| Failure | Fallback                                                                                                                                                           |
| ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| W2      | Find the largest honored timeout by bisection (1,800 / 1,200 / 900 s), set the hook to it, keep the "not listening" hint, point long reviews at `wait_for_review`. |
| W4, W5  | Document the limitation in `docs/agents.md`; the Submit dialog hint tells the human to nudge; open a follow-up design question for session linking.                |
| W1      | Blocks v1 (G3). Re-check the hook JSON against current Claude Code docs, then escalate.                                                                            |

## Cleanup

After the last case, quit Claude Code and Polygloss. In each scratch repo you installed the plugin in:

```bash
claude plugin uninstall polygloss@polygloss --scope local
```

Then once:

```bash
claude plugin marketplace remove polygloss
rm -rf /tmp/polygloss-wake-gate
```
