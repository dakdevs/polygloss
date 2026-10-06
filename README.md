# Polygloss

Polygloss is a local code review app for macOS. Open any diff (your working tree, a commit, or a branch compared like a pull request) and review it the way you would on GitHub: mark files Viewed, leave threaded comments, then submit the review. Your coding agent reads it over MCP and gets back to work.

Your code never leaves your machine. There are no accounts and no server, and the only network access is an optional update check.

## Why?

Agents change a lot of code quickly. Reading those changes in a terminal is painful, and pushing to GitHub just to read a diff is slow. Polygloss brings GitHub's review flow to your own machine and closes the loop with the agent: when you submit a review, Claude Code wakes up, reads your comments, fixes the code, and asks you to look again.

## Installation

Download `Polygloss_<version>_aarch64.dmg` from the [latest release](https://github.com/dakdevs/polygloss/releases/latest), open it and drag Polygloss to Applications, then run "Install CLI" from ⌘K. You need macOS 14 or later on Apple Silicon and git 2.39 or later. Releases are signed and notarized, and update in place.

Or build from source with [rustup](https://rustup.rs), Bun 1.3 and cargo-packager 0.11.8 ([details](CONTRIBUTING.md#setup)):

```bash
git clone https://github.com/dakdevs/polygloss && cd polygloss
bun install --frozen-lockfile
scripts/package-release.sh   # builds dist/Polygloss.app
open dist/Polygloss.app      # then run "Install CLI" from ⌘K
```

To connect Claude Code, install the plugin. It adds the MCP server and the hook that wakes Claude when you submit:

```bash
claude plugin marketplace add dakdevs/polygloss
claude plugin install polygloss@polygloss
```

Other agents can run `polygloss mcp` as a stdio MCP server or use the JSON CLI ([docs/agents.md](docs/agents.md)).

## Usage

```bash
polygloss                       # working tree against its merge-base
polygloss show HEAD             # one commit
polygloss compare main feature  # a branch, like a pull request
```

Or ask Claude to "open this in Polygloss for review". In the app, `⌘K` lists every action and `?` every key.

## Some notes

Polygloss is early. It works end to end, but expect rough edges.

## Documentation

- [User guide](docs/user-guide.md): reviewing, comments, Viewed, live mode, keys, settings, themes
- [Agents](docs/agents.md): the Claude Code plugin, MCP tools, the JSON CLI
- [Design](docs/design.md) and [decision records](docs/adr/README.md)

## Development

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. Run cargo only through `scripts/cargo.sh`, because a Homebrew `cargo` earlier on `PATH` breaks the build. Before sending a change, run:

```bash
bun install --frozen-lockfile
bun run format
bun run lint
bun run test:unit
bun test
bun run test:e2e
```

## License

`MIT OR Apache-2.0`, at your option. See [NOTICE](NOTICE) for third-party credits.
