# ADR-0019: Distribution and signing

- **Status:** Accepted; amended 2026-10-06 (CalVer versions, a release for every push to `main`, release notes from the diff)
- **Date:** 2026-09-28
- **Design:** [§21 Packaging and distribution](../design.md#21-packaging-and-distribution)

## Context

The user has an Apple Developer ID. A native app with a CLI and git subprocesses has to be signed and notarized. The Mac App Store sandbox would break git subprocesses, unix sockets and installing the CLI. The user's "shows" app already uses team 5U7E4UQ5M3 with bundle id `dev.dak.shows`.

## Decision

- Sign with the **Developer ID** of team **5U7E4UQ5M3**, bundle id **`dev.dak.polygloss`**.
- Build a signed and notarized DMG with **cargo-packager 0.11.8** in CI, and publish it on public **GitHub Releases**.
- **Versions are CalVer `YYYYMMDD.N`** (amended 2026-10-06): the release date in America/Los_Angeles, and N one more than the highest N among that date's tags (`20261005.1`, `20261005.2`, … `20261005.10`, `20261006.1`). The tag is `v<version>`; the release is titled `<version>`. Both `Info.plist` version keys, the DMG name, `--version`, the app socket's `hello` and MCP `serverInfo`, the appcast and the cask carry it. Sparkle's comparator orders it numerically per component. Crate versions stay semver and local builds report them.
- **Every push to `main` whose CI jobs all pass is released** (amended 2026-10-06): `ci.yml`'s `release` job needs every CI job and calls `release.yml`, which runs one release at a time, computes N from origin's tags inside that group, signs from a temporary keychain, notarizes, smoke-tests the bundle, then creates the tag and the latest release with the DMG, `appcast.xml` and `SHA256SUMS`. A missing signing, notarization or Sparkle secret fails the job before anything is built: there is no unsigned release. The job's actions are pinned to commits and it installs no packages, since anything that runs before signing could rewrite the signing scripts. A commit that does not come after the latest release tag in history (a later push released first, or a re-run of an older or released commit) is skipped with a notice, so the latest release never goes back in time. `workflow_dispatch` re-runs a release from `main` or, by default, makes a dry run, which is ad-hoc signed and never reads the secrets. This supersedes the `v*` tag trigger and the check that a tag equals the crate version (T5.2).
- **Release notes come from the diff** (amended 2026-10-06): `scripts/release-notes.ts` groups the subjects of the commits since the previous release tag by conventional-commit type (Breaking changes, Features, Fixes, Performance, Changes, Documentation, and Maintenance folded, which also takes every type with an internal scope such as `ci`, `release`, `plan` or a milestone), without plan ids such as `T6.15`, after a summary line of commits, files and lines changed, and links each commit and the compare view. It reads only git, so a commit always gets the same notes. They are the GitHub Release body and are embedded in the appcast for Sparkle's update dialog; Sparkle's copy leaves Maintenance unfolded, because Sparkle's Markdown renderer prints `<details>` as text.
- Offer a personal **Homebrew tap** cask whose `binary` stanza puts `polygloss` on PATH, plus an in-app **Install CLI** command.
- Update through **Sparkle**, using objc2 FFI, with the appcast on GitHub Releases (amended 2026-10-06; Pages is not used).
- The **Claude Code plugin marketplace** lives in the same repo (`claude plugin marketplace add <user>/polygloss`). The plugin calls the CLI through a stable symlink path.
- `polygloss mcp` runs from the separate slim CLI binary.
- The Mac App Store is ruled out.

## Consequences

- ~~**Release blocker:** as of 2026-09-28 the keychain holds only an _Apple Distribution_ cert for 5U7E4UQ5M3.~~ Resolved 2026-10-05: a Developer ID Application certificate for 5U7E4UQ5M3 and an App Store Connect API key exist, both backed up in 1Password. The Developer ID Application certs of team FCSF68W94H in the keychain must still never be used.
- A Sparkle EdDSA key lives in CI secrets. Sparkle checks are the one network access (OQ-16).
- The appcast URL defaults to `https://github.com/dakdevs/polygloss/releases/latest/download/appcast.xml` (repository variable `POLYGLOSS_APPCAST_URL` overrides it). Release downloads and Sparkle's update checks are unauthenticated, so they work only while the repository is public; a private repository's release assets need a GitHub login.
- `scripts/setup-release-secrets.sh` sets the secrets and the variable `release.yml` needs from 1Password, and makes the Sparkle key pair (saved in 1Password too) when there is none.
- A pending release that a newer push to `main` supersedes is skipped (GitHub keeps one pending run per concurrency group); the newer release contains its commits.
- Both executables are signed. Install CLI is written from scratch; Zed's is GPL.
- As built (plan T5.1): cargo-packager lays out the `.app`; `scripts/package-release.sh` stamps `CFBundleVersion`, signs (ad-hoc without credentials) and makes the DMG with `hdiutil`, because cargo-packager's DMG step downloads `create-dmg` at build time and would package the app before it is signed.
- As built (plan T5.2): `scripts/sign-and-notarize.sh` signs the app inside out and then the DMG, notarizes with `notarytool` and staples, all from environment credentials. It refuses any identity that is not a Developer ID Application of team 5U7E4UQ5M3 (team FCSF68W94H by name) before touching a keychain, imports a CI certificate into a temporary keychain and restores the user's keychain search list on exit. Without credentials it skips and the artifacts stay ad-hoc signed. `.github/workflows/release.yml` reads secrets only through step `env:`; it ran on `v*` tags until the 2026-10-06 amendment.
- As built (plan T5.3): Sparkle 2.10.0 is loaded at runtime (`polygloss_platform::sparkle`), never linked. `scripts/fetch-sparkle.sh` vendors the release (SHA-256 pinned and checked against the digest GitHub publishes, XPC services removed). A release embeds it only when `POLYGLOSS_APPCAST_URL` and `SPARKLE_PUBLIC_ED_KEY` are set, which write `SUFeedURL`/`SUPublicEDKey`; otherwise it has no updater. `SUEnableAutomaticChecks` stays unset, so Sparkle asks before any automatic check. Library validation rejects an ad-hoc framework in an ad-hoc app, so ad-hoc bundles that embed Sparkle carry `packaging/entitlements-adhoc.plist` (`disable-library-validation`); the Developer ID signature replaces it with the exception-free `packaging/entitlements.plist`, and `scripts/smoke-bundle.sh` fails a Developer ID bundle that keeps it.
- As built (plan T5.4): the tap cask is rendered from `packaging/homebrew/polygloss.rb.tmpl` by `scripts/bump-tap.ts` after each release (on release tags until the 2026-10-06 amendment). Install CLI links `/usr/local/bin/polygloss` to the bundle's `polygloss-cli`, replacing only symlinks; without write access it asks for an administrator password through `osascript`, with every path passed as an argument and shell-quoted by the script, never spliced into it.
- Amended 2026-10-06 (app icon): the release no longer runs actool. Xcode 26.3's actool crashed on the runner (its `ibtoold` helper failed to load), failing a release that a dry run on the same image had passed. `scripts/make-icon.sh` compiles `packaging/polygloss.icon` locally into the committed `packaging/assets.car`, with a manifest of the document's and the catalog's SHA-256; `package-release.sh` bundles it only while that manifest matches, and `POLYGLOSS_REQUIRE_APP_ICON=1` fails a release whose copy is stale.

## Alternatives rejected

| Option                            | Why not                                               |
| --------------------------------- | ----------------------------------------------------- |
| Mac App Store                     | The sandbox breaks git, sockets and the CLI install   |
| The official `homebrew/cask` repo | Requires a notable project; start with a personal tap |
| cargo-packager-updater            | Not published since 2025-07; no relaunch              |
| Signing with team FCSF68W94H      | Wrong team                                            |
