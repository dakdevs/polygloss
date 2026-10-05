# Polygloss user guide

Polygloss is a native macOS app for reviewing diffs on your own machine, especially changes written by coding agents. It works like GitHub's "Files changed" tab: split and unified diffs, word highlights, a **Viewed** checkbox per file, threaded comments you draft and then publish in one **Submit review**. Everything stays local: Polygloss reads repositories already on disk, keeps its state in one SQLite file, and never fetches or talks to a forge.

This guide covers the app. For agents (MCP tools, the JSON CLI, the Claude Code plugin) see [agents.md](agents.md). Installation is in the [README](../README.md#install).

## Contents

- [Opening a review](#opening-a-review)
- [Reviewing](#reviewing)
- [Comments, drafts and Submit review](#comments-drafts-and-submit-review)
- [Viewed](#viewed)
- [Live mode](#live-mode)
- [Iterations and re-reviews](#iterations-and-re-reviews)
- [Finding things](#finding-things)
- [Open in editor](#open-in-editor)
- [Notifications and the Dock badge](#notifications-and-the-dock-badge)
- [Updates](#updates)
- [Keyboard](#keyboard)
- [Settings](#settings)
- [Themes and fonts](#themes-and-fonts)
- [Files on disk](#files-on-disk)

## Opening a review

A **review** is one review intent inside a repository, such as "my working tree since the merge-base" or "feature vs main". It keeps its comments, Viewed marks and verdicts across **iterations**: each time the diff it shows changes and gets pinned, a new iteration is recorded (Iteration 1, 2, 3 and so on).

There are three kinds of diff:

| Source                      | What it shows                                                                                                                       | From the terminal                                                |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| **Live** worktree           | Your working tree (tracked and untracked files, not ignored ones) against a base: the merge-base with the default branch by default | `polygloss [--since merge-base\|HEAD\|<rev>] [<path>]`           |
| **Commit**                  | One commit against its first parent (a merge commit against its first parent; a root commit against the empty tree)                 | `polygloss show <rev>`                                           |
| **Branch compare** ("a PR") | `head` against `base`, three-dot (the changes on `head` since the merge-base) by default, or `--direct` to compare the two trees    | `polygloss compare <base> <head> [--direct] [--label "PR #123"]` |

In the app, **⌘O** opens the open flow: pick a repository (recent ones first, or **Browse…**), then Live (with its base), a commit from the log, or a branch compare (base and head pickers, a direct toggle and an optional label). Lists are fuzzy-filtered as you type.

`polygloss open <diff_id>` opens a diff by its id (or a unique prefix of at least 8 hex digits), and `polygloss://diff/<diff_id>` links do the same. Pull requests are branch compares of refs you already have locally: run `gh pr checkout` or `git fetch` yourself first, since Polygloss never fetches.

### Home

The first tab lists recent reviews across all repositories, most recently active first, in two sections: **Awaiting you** (an agent asked for a re-review, or an agent question is still unanswered) and **Recent**. Each row shows the repository, title, kind, status or last verdict, Viewed progress, open threads and the agent the review is assigned to.

Select a row with `j`/`k` or the arrows and press `⏎` to open it. The row's ⋯ menu (or a right click) also offers archive (`e`), mute (`m`), "Assign to session…" (`a`) and prune (`⌘⌫`, after a confirmation). Pruning deletes the review and its comments from Polygloss; it never touches your repository. With `storage.prune_reviews_after_days` set, stale reviews are pruned at launch and once a day, except reviews with drafts, reviews awaiting you and reviews whose repository is gone.

## Reviewing

A review tab has a toolbar, a banner strip, and three resizable panes: the **file tree**, the **diff**, and the **threads panel** (toggle it from the palette or the View menu).

- **Toolbar:** the iteration picker ("Iteration 3 of 3") with **Changes since last review**, the base picker and **Snapshot** (live reviews), split/unified, hide whitespace, word diff by words or characters, "N / M viewed", **Hide agent notes**, and the drafts count with **Submit review**.
- **Layout:** split when the diff is at least 160 code columns wide (`diff.split_min_columns`), otherwise unified. `s` switches, and the choice is remembered for that diff.
- **Word diff:** each modified line pair highlights the changed words (or characters).
- **Context:** 3 lines around each change. The gap expanders show 20 more lines up or down, or everything; `e` expands the gap nearest the cursor by 20 lines and `⇧E` expands the whole file.
- **Line cursor:** `j`/`k` (or the arrows) move a line cursor across rows and files; it is where `c`, `o` and `e` act. `⇧↓`/`⇧↑` extend it to a range on one side.
- **File headers** stay pinned at the top while you scroll through a file. They show the path (`old → new` for renames), +/− counts, badges (mode change, binary, symlink, submodule, generated, LFS), the Viewed checkbox, a collapse chevron and a ⋯ menu (Open in editor, Comment on file, Copy path, Expand all, Load diff).
- **Large and generated files** start collapsed behind **Load diff**: files with more than `diff.large_file_changed_lines` changed lines, and generated files: those marked `linguist-generated` in `.gitattributes`, or matching the built-in list (lock files, minified and source-map files, protobuf output) or `diff.generated_patterns`.
- **File tree:** checkboxes mark files viewed (folders show a combined state and offer "Mark folder viewed"), with status letters, +/− counts, thread and agent badges, and a dot for files changed since you viewed them. Filter by unviewed, has comments, status or extension, or type to fuzzy-filter. Selecting a file scrolls the diff to it, and scrolling highlights the current file.
- **Selection and copy:** drag to select text on one side; `⌘C` copies the source text without gutters or `+`/`-` markers.

Where you were (scroll position, collapsed files, expanded context, layout, tree folders) is saved per diff and restored when you open it again.

## Comments, drafts and Submit review

Threads anchor to lines of one side of the diff (a line or a range), to a whole file, or to the whole review. Line numbers are lines of the file on that side (old = base, new = head), not positions in the hunk, so you can comment on any displayed line, context lines included.

1. **Comment:** press `c` on the cursor line or selection, click the **+** that appears next to a line number, or drag across line numbers for a range. "Comment on file" is in the file header's ⋯ menu, and "Comment on review" is in the threads panel; both are in the command palette too.
2. **Write:** comments are markdown, with a Write/Preview toggle. A ` ```suggestion ` block proposes replacement text for the anchored new-side lines; it shows as a small diff, and the agent applies it (there is no Apply button).
3. **Save a draft:** `⌘⏎`. `Esc` cancels. Text you have not saved yet is kept if you switch tabs or iterations. Your new threads and replies are **drafts** with a Draft badge until you submit: agents cannot see them.
4. **Submit review:** `⇧⌘⏎`, or the toolbar button showing the draft count. Add an optional summary and pick a verdict: **Request changes**, **Comment** or **Approve**. Submitting publishes every draft at once and wakes the agent the review is assigned to. You can submit with no drafts, for example just to approve. The dialog says whether the agent is listening right now; if it is not, it sees the review on its next turn.

Threads are **open** or **resolved**; you and agents can both resolve and reopen them, and that takes effect at once (it is not a draft). You can edit and delete your own comments.

Agents write three kinds of threads. Their **comments and replies** are published immediately, and a banner ("claude-code replied to N threads") jumps to the next unread one. **Notes** explain code: they show as one-line chips, and **Hide agent notes** hides them all (`agent_notes.hidden` sets the default). **Questions** ask you to decide something: they show expanded with a question badge and count as "awaiting you" until you reply (and submit) or resolve them.

When the code under a thread changes in a later iteration, the thread follows its lines. If the lines moved, it moves with them. If they changed, it is shown as **Outdated** at the nearest line with its original snippet, and it is also listed in the threads panel. It stays open until someone resolves it. `.` and `,` jump to the next and previous open thread.

## Viewed

Mark a file **Viewed** with its header checkbox, its tree checkbox, or `v`. Marking a file viewed collapses it and jumps to the next unviewed file. The toolbar counts "N / M viewed".

- Viewed belongs to the file's exact change: its path plus the old and new file contents. A file stays viewed across iterations, and even across reviews, for as long as that change is the same.
- When the file changes again, it unchecks itself and shows **Changed since viewed** (the tree marks it with a dot), like GitHub's dismissed state.
- A rebase that moves only the base still changes the old side, so the file becomes unviewed.
- Agents can read the Viewed counts but never set them.

## Live mode

A live review watches its worktree. When files change, a banner says **"N files changed · Refresh (R)"** (or that the base moved, when HEAD or the merge-base changed). Nothing changes under you until you press `⇧R` or click the banner; not even the agent's own edits are applied automatically. Refreshing keeps your place, collapsed files, expanded context and Viewed marks.

- **Base picker:** merge-base with the default branch (the default), `HEAD`, or a fixed commit from the log. Each base is its own review. With merge-base, commits the agent makes do not change the diff (it shows the branch plus uncommitted work); with `HEAD`, a commit moves the base. The default branch is `origin/HEAD`, else `origin/main`, `origin/master`, `main`, `master`, else `HEAD` with a notice.
- **Snapshot** pins the current state as a new iteration. Adding a comment or draft pins it too, as do submitting and an agent opening the diff, commenting or asking for a re-review. Pinned states are kept alive with refs under `refs/polygloss/`; Polygloss never touches your index, HEAD or branches.
- **Branch compares** watch their two refs and offer "New iteration available · Refresh (R)" when either moves.

Watchers keep running while the app is in the background, so the banner count keeps growing.

## Iterations and re-reviews

The iteration picker switches between the review's iterations. **Changes since last review** shows only what changed since the iteration you last submitted on (comments on it go on the new side). When an agent asks for a re-review, the review shows **Ready for re-review** with the agent's summary, it moves to Home's **Awaiting you**, and the banner offers "View changes since last review".

## Finding things

- **⌘K** command palette: every action, with its key.
- **⌘P** go to file, fuzzy.
- **⌘F** find in all files, including ones not loaded yet. Results stream into a list with a count; `⏎`/`⇧⏎` go to the next and previous match, and every visible match is highlighted in the diff. Case-sensitive and regex toggles are in the find bar.
- **?** keyboard shortcuts cheat sheet.

## Open in editor

`o` (or the file header's ⋯ menu) opens the cursor line in your editor. A new-side line of a file that exists on disk opens the current file at the matching line, even if it has changed since the diff. An old-side line, a deleted file or a file missing on disk opens a read-only temporary copy.

The editor is `editor.command` when set, a template such as `"zed {path}:{line}"` or `"code -g {path}:{line}"`; otherwise Zed, Cursor or VS Code (the first installed), then `$VISUAL`, then `$EDITOR`. The command runs directly, never through a shell.

## Notifications and the Dock badge

When an agent asks for a re-review, Polygloss shows a macOS notification if the app is not focused (it starts in the background to deliver it if needed). Clicking the notification brings up the review's tab. The Dock badge counts reviews awaiting you. Mute a review from its Home row (`m`), or turn notifications off everywhere with `notifications.enabled: false`.

## Updates

Releases built with an update feed check for new versions through [Sparkle](https://sparkle-project.org), the only network access Polygloss makes. Nothing is checked until you agree: Sparkle asks on the second launch whether to check automatically. **Polygloss › Check for Updates…** (also in the command palette) checks now. `updates.automatic_checks` overrides your answer (`true` or `false`); left at `null`, Sparkle's prompt decides. Builds without a feed (from source, or a release built without one) have no updater and no menu item.

## Keyboard

Every action has a key or a command palette entry, and every key can be remapped. Single-key bindings (`j`, `?`, `⇧R`) never fire while you are typing in a text field or a comment. Keys are shown as key caps: `J` is the J key alone, and `⇧E` adds Shift.

A binding's context says where it works: **Viewport** (the diff has focus), **Tree** (the file tree has focus), **Composer** (a comment box has focus), **ThreadsPanel** (the threads panel has focus), **Tab** (anywhere in a review tab), **Window** (anywhere in the main window) or **Anywhere**.

Everything works without a mouse. In a review tab, `⇥` and `⇧⇥` move the keyboard between panes: file tree, diff, threads panel (when shown), then any open comment boxes. The pane with the keyboard shows a focus ring while you use the keyboard. In a comment box and the Submit review summary, `⇥` moves on instead of indenting; `⌘]` and `⌘[` indent and outdent. In the Submit review dialog, `⌘1`, `⌘2` and `⌘3` pick **Comment**, **Approve** and **Request changes**. On Home, `⇧R` reloads the list. `Esc` closes every dialog, popover and menu and gives the keyboard back.

### Default key bindings

| Keys    | `keymap.json`     | Action                       | Name                            | Context      |
| ------- | ----------------- | ---------------------------- | ------------------------------- | ------------ |
| `J`     | `j`               | Move cursor down             | `viewport::CursorDown`          | Viewport     |
| `↓`     | `down`            | Move cursor down             | `viewport::CursorDown`          | Viewport     |
| `K`     | `k`               | Move cursor up               | `viewport::CursorUp`            | Viewport     |
| `↑`     | `up`              | Move cursor up               | `viewport::CursorUp`            | Viewport     |
| `⇧↓`    | `shift-down`      | Extend selection down        | `viewport::ExtendSelectionDown` | Viewport     |
| `⇧↑`    | `shift-up`        | Extend selection up          | `viewport::ExtendSelectionUp`   | Viewport     |
| `N`     | `n`               | Next file                    | `viewport::NextFile`            | Viewport     |
| `P`     | `p`               | Previous file                | `viewport::PrevFile`            | Viewport     |
| `N`     | `n`               | Next file in tree            | `tree::NextFile`                | Tree         |
| `P`     | `p`               | Previous file in tree        | `tree::PrevFile`                | Tree         |
| `]`     | `]`               | Next change                  | `viewport::NextChange`          | Viewport     |
| `[`     | `[`               | Previous change              | `viewport::PrevChange`          | Viewport     |
| `V`     | `v`               | Toggle viewed                | `viewport::ToggleViewed`        | Viewport     |
| `V`     | `v`               | Toggle viewed in tree        | `tree::ToggleViewed`            | Tree         |
| `C`     | `c`               | Comment on line or selection | `viewport::Comment`             | Viewport     |
| `⌘⏎`    | `cmd-enter`       | Save draft                   | `composer::SaveDraft`           | Composer     |
| `.`     | `.`               | Next open thread             | `viewport::NextOpenThread`      | Viewport     |
| `,`     | `,`               | Previous open thread         | `viewport::PrevOpenThread`      | Viewport     |
| `E`     | `e`               | Expand context               | `viewport::ExpandContext`       | Viewport     |
| `⇧E`    | `shift-e`         | Expand whole file            | `viewport::ExpandFile`          | Viewport     |
| `S`     | `s`               | Toggle split / unified       | `viewport::ToggleLayout`        | Viewport     |
| `W`     | `w`               | Toggle hide whitespace       | `viewport::ToggleWhitespace`    | Viewport     |
| `⇧R`    | `shift-r`         | Refresh                      | `tab::Refresh`                  | Tab          |
| `O`     | `o`               | Open in editor               | `viewport::OpenInEditor`        | Viewport     |
| `⌘P`    | `cmd-p`           | Go to file                   | `window::FileFinder`            | Window       |
| `⌘K`    | `cmd-k`           | Command palette              | `window::CommandPalette`        | Window       |
| `⌘O`    | `cmd-o`           | Open review                  | `window::OpenFlow`              | Window       |
| `⌘F`    | `cmd-f`           | Find in all files            | `tab::Find`                     | Tab          |
| `⇧⌘⏎`   | `cmd-shift-enter` | Submit review                | `tab::SubmitReview`             | Tab          |
| `?`     | `?`               | Keyboard shortcuts           | `window::CheatSheet`            | Window       |
| `Esc`   | `escape`          | Cancel comment               | `composer::Cancel`              | Composer     |
| `⌘C`    | `cmd-c`           | Copy selection               | `viewport::Copy`                | Viewport     |
| `⌘W`    | `cmd-w`           | Close tab                    | `window::CloseTab`              | Anywhere     |
| `⌘}`    | `cmd-}`           | Next tab                     | `window::NextTab`               | Anywhere     |
| `⌘{`    | `cmd-{`           | Previous tab                 | `window::PrevTab`               | Anywhere     |
| `⌃Tab`  | `ctrl-tab`        | Next tab                     | `window::NextTab`               | Anywhere     |
| `⌃⇧Tab` | `ctrl-shift-tab`  | Previous tab                 | `window::PrevTab`               | Anywhere     |
| `⌘,`    | `cmd-,`           | Open settings                | `window::OpenSettings`          | Window       |
| `⌘Q`    | `cmd-q`           | Quit Polygloss               | `window::Quit`                  | Anywhere     |
| `⌘M`    | `cmd-m`           | Minimize                     | `window::Minimize`              | Anywhere     |
| `Tab`   | `tab`             | Focus next pane              | `tab::FocusNextPane`            | Tab          |
| `⇧Tab`  | `shift-tab`       | Focus previous pane          | `tab::FocusPrevPane`            | Tab          |
| `I`     | `i`               | Choose iteration             | `tab::ChooseIteration`          | Tab          |
| `M`     | `m`               | File menu                    | `viewport::FileMenu`            | Viewport     |
| `Z`     | `z`               | Collapse or expand file      | `viewport::ToggleCollapse`      | Viewport     |
| `/`     | `/`               | Filter files                 | `tree::FocusFilter`             | Tree         |
| `F`     | `f`               | File filters                 | `tree::FilterMenu`              | Tree         |
| `J`     | `j`               | Next thread in panel         | `threads::SelectNext`           | ThreadsPanel |
| `↓`     | `down`            | Next thread in panel         | `threads::SelectNext`           | ThreadsPanel |
| `K`     | `k`               | Previous thread in panel     | `threads::SelectPrev`           | ThreadsPanel |
| `↑`     | `up`              | Previous thread in panel     | `threads::SelectPrev`           | ThreadsPanel |
| `⏎`     | `enter`           | Go to thread                 | `threads::Open`                 | ThreadsPanel |
| `R`     | `r`               | Reply to thread              | `threads::Reply`                | ThreadsPanel |
| `X`     | `x`               | Resolve or unresolve thread  | `threads::ToggleResolved`       | ThreadsPanel |
| `E`     | `e`               | Edit my comment              | `threads::EditComment`          | ThreadsPanel |

Popovers and dialogs also close with `Esc`.

### Actions without a default key

These are in the command palette (and some in the toolbar or menus); bind them in `keymap.json` if you use them often.

| Action                    | Name                                |
| ------------------------- | ----------------------------------- |
| Split view                | `viewport::LayoutSplit`             |
| Unified view              | `viewport::LayoutUnified`           |
| Automatic layout          | `viewport::LayoutAuto`              |
| Word diff: words          | `viewport::WordDiffWord`            |
| Word diff: characters     | `viewport::WordDiffChar`            |
| Word diff: off            | `viewport::WordDiffOff`             |
| Mark folder viewed        | `tree::MarkFolderViewed`            |
| Toggle comment preview    | `composer::TogglePreview`           |
| Delete my comment         | `threads::DeleteComment`            |
| Snapshot                  | `tab::Snapshot`                     |
| Choose base               | `tab::ChooseBase`                   |
| Comment on file           | `tab::CommentOnFile`                |
| Comment on review         | `tab::CommentOnReview`              |
| Assign to session         | `tab::AssignToSession`              |
| Hide agent notes          | `tab::ToggleAgentNotes`             |
| Changes since last review | `tab::ToggleChangesSinceLastReview` |
| Next unread reply         | `tab::NextUnreadThread`             |
| Toggle threads panel      | `tab::ToggleThreadsPanel`           |
| Install CLI               | `window::InstallCli`                |
| Zoom                      | `window::Zoom`                      |
| Check for updates         | `window::CheckForUpdates`           |

### Remapping keys

`~/.config/polygloss/keymap.json` (or `$XDG_CONFIG_HOME/polygloss/keymap.json`) holds your overrides, as a list of sections with an optional context:

```jsonc
[
  {
    "context": "Viewport",
    "bindings": {
      "ctrl-n": "viewport::NextFile", // add a key
      "shift-v": "tree::MarkFolderViewed",
      "w": null, // unbind a default key
    },
  },
  { "bindings": { "cmd-shift-s": "tab::Snapshot" } }, // no context: everywhere
]
```

A binding replaces the default of the same keys in the same context, and `null` unbinds the keys there. Keys use the spelling of the `keymap.json` column above (`cmd`, `ctrl`, `alt`, `shift`, joined with `-`; `shift-e` is `E`); a sequence is keys separated by spaces. Contexts may combine names with `&&`, `||` and `!`. Comments and trailing commas are allowed. The file reloads when you save it; if it has an error (bad JSON, an unknown action, a bad key or context), the previous bindings stay and the window shows the error.

## Settings

`⌘,` opens `~/.config/polygloss/settings.json` (or `$XDG_CONFIG_HOME/polygloss/settings.json`) in your editor. Every key is optional: a missing key keeps its default and unknown keys are ignored. Comments and trailing commas are allowed. The file reloads when you save it; a value of the wrong type or out of range makes the whole file invalid, the previous settings stay, and the window shows the error.

```jsonc
{
  "theme": { "mode": "dark", "dark": "One Dark" },
  "buffer_font": { "family": "Lilex", "size": 14 },
  "diff": {
    "layout": "split",
    "style": { "indicators": "bars", "wrap": true },
  },
  "editor": { "command": "zed {path}:{line}" },
}
```

### Settings keys

| Key                                | Default             | Meaning                                                                                                                                                                                                      |
| ---------------------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `theme.mode`                       | `"system"`          | `"system"` follows the macOS appearance; `"light"` or `"dark"` fixes it                                                                                                                                      |
| `theme.light`                      | `"Polygloss Light"` | The theme used in light mode, by name                                                                                                                                                                        |
| `theme.dark`                       | `"Polygloss Dark"`  | The theme used in dark mode, by name                                                                                                                                                                         |
| `buffer_font.family`               | `"Lilex"`           | The code font (Lilex is bundled)                                                                                                                                                                             |
| `buffer_font.size`                 | `13`                | Code font size in points                                                                                                                                                                                     |
| `buffer_font.ligatures`            | `false`             | OpenType ligatures in code. Off, so code shows as typed: `->` is never drawn as an arrow                                                                                                                     |
| `diff.layout`                      | `"auto"`            | `"auto"` (split when wide enough), `"split"` or `"unified"`                                                                                                                                                  |
| `diff.split_min_columns`           | `160`               | In auto layout, split from this many code columns                                                                                                                                                            |
| `diff.word_diff`                   | `"word"`            | Changed-text highlights by `"word"`, by `"char"`, or `"off"`                                                                                                                                                 |
| `diff.algorithm`                   | `"myers"`           | `"myers"` (like `git diff`) or `"histogram"`                                                                                                                                                                 |
| `diff.hide_whitespace`             | `false`             | Hide whitespace-only changes                                                                                                                                                                                 |
| `diff.style.backgrounds`           | `true`              | Tint added and removed lines                                                                                                                                                                                 |
| `diff.style.indicators`            | `"+-"`              | Line markers: `"+-"`, `"bars"` or `"none"`                                                                                                                                                                   |
| `diff.style.wrap`                  | `false`             | Wrap long lines                                                                                                                                                                                              |
| `diff.large_file_changed_lines`    | `20000`             | Files with more changed lines start collapsed behind **Load diff**                                                                                                                                           |
| `diff.generated_patterns`          | `[]`                | More generated-file patterns, added to the built-in list: a pattern without `/` matches the file name, one with `/` the whole path; `*` and `?` stay within a directory, `**` crosses directories            |
| `diff.renames`                     | `true`              | Detect renames                                                                                                                                                                                               |
| `diff.rename_threshold`            | `50`                | Rename similarity threshold, in percent (0 to 100)                                                                                                                                                           |
| `editor.command`                   | `null`              | The editor for [Open in editor](#open-in-editor), e.g. `"zed {path}:{line}"`; `null` detects one                                                                                                             |
| `agent_notes.hidden`               | `false`             | Hide agent notes by default                                                                                                                                                                                  |
| `notifications.enabled`            | `true`              | macOS notifications for re-review requests (agents' `polygloss mcp` reads it too, and never launches the app to notify when it is off)                                                                       |
| `storage.prune_reviews_after_days` | `null`              | Prune reviews inactive for this many days; `null` never prunes                                                                                                                                               |
| `updates.automatic_checks`         | `null`              | Automatic update checks ([Updates](#updates)): `null` leaves it to Sparkle's prompt on the second launch, `true` or `false` overrides your answer. Update checks are the only network access Polygloss makes |

## Themes and fonts

Polygloss uses **Polygloss Light** and **Polygloss Dark** by default and follows the system appearance. **Pierre Light** and **Pierre Dark** are bundled too. Any [Zed](https://zed.dev) theme file (JSON) dropped into `~/.config/polygloss/themes/` is loaded as well; a file may hold several themes. Pick one by name with `theme.light` / `theme.dark` (and `theme.mode` to fix the appearance). An unknown name falls back to Polygloss of that appearance; a broken file is skipped with an error. Themes reload when the folder changes.

A theme colors the interface, the syntax highlighting and the diff (its created, deleted and modified colors). A few colors have no Zed key; a theme may set them as `polygloss.*` style keys, and a theme without them gets colors derived from its standard keys:

| Key                                                                          | Colors                                           | Derived from                                             |
| ---------------------------------------------------------------------------- | ------------------------------------------------ | -------------------------------------------------------- |
| `polygloss.sidebar.field.background`                                         | The sidebar's filter field and segmented control | `element.background`                                     |
| `polygloss.created.line_number`, `polygloss.deleted.line_number`             | Line numbers of changed rows                     | `created`, `deleted`                                     |
| `polygloss.created.gutter_background`, `polygloss.deleted.gutter_background` | Behind those line numbers                        | `created.background`, `deleted.background`, a bit deeper |
| `polygloss.stat.added`, `polygloss.stat.deleted`                             | `+a −d` counts                                   | `version_control.added`, `version_control.deleted`       |
| `polygloss.commit_sha`                                                       | Commit SHAs                                      | syntax `type.builtin`, then `terminal.ansi.yellow`       |

Code uses the bundled Lilex font by default; set `buffer_font.family` to another installed family, or to `"SF Mono"` or `"System Mono"` for the system's monospaced font. A family that is not installed shows in the system monospaced font. The interface uses the system font.

## Files on disk

| Path                                       | Holds                                                                                                          |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------- |
| `~/Library/Application Support/polygloss/` | `polygloss.db` (reviews, comments, Viewed marks), the app socket, and `bin/polygloss`, a link to the app's CLI |
| `~/Library/Caches/polygloss/`              | Unpinned live snapshots and read-only copies for Open in editor                                                |
| `~/Library/Logs/polygloss/`                | The app's log                                                                                                  |
| `~/.config/polygloss/`                     | `settings.json`, `keymap.json`, `themes/`                                                                      |
| `/usr/local/bin/polygloss`                 | Made by **Install CLI** (command palette or the Polygloss menu): a link to the app's CLI, for DMG installs     |
| `refs/polygloss/` in each repository       | Refs that keep pinned snapshots alive                                                                          |

`POLYGLOSS_DATA_DIR` moves the data directory (the cache and logs then go inside it, unless `POLYGLOSS_CACHE_DIR` or `POLYGLOSS_LOG_DIR` say otherwise), and `XDG_CONFIG_HOME` moves the config directory. `POLYGLOSS_LOG` sets the app's log filter (for example `POLYGLOSS_LOG=debug`).
