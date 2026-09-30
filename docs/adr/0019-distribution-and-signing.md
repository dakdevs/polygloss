# ADR-0019: Distribution and signing

- **Status:** Accepted, with a release blocker (see Consequences)
- **Date:** 2026-09-28
- **Design:** [§21 Packaging and distribution](../design.md#21-packaging-and-distribution)

## Context

The user has an Apple Developer ID. A native app with a CLI and git subprocesses has to be signed and notarized. The Mac App Store sandbox would break git subprocesses, unix sockets and installing the CLI. The user's "shows" app already uses team 5U7E4UQ5M3 with bundle id `dev.dak.shows`.

## Decision

- Sign with the **Developer ID** of team **5U7E4UQ5M3**, bundle id **`dev.dak.polygloss`**.
- Build a signed and notarized DMG with **cargo-packager 0.11.8** in CI, and publish it on public **GitHub Releases**.
- Offer a personal **Homebrew tap** cask whose `binary` stanza puts `polygloss` on PATH, plus an in-app **Install CLI** command.
- Update through **Sparkle**, using objc2 FFI, with the appcast on Releases or Pages.
- The **Claude Code plugin marketplace** lives in the same repo (`claude plugin marketplace add <user>/polygloss`). The plugin calls the CLI through a stable symlink path.
- `polygloss mcp` runs from the separate slim CLI binary.
- The Mac App Store is ruled out.

## Consequences

- **Release blocker:** as of 2026-09-28 the keychain holds only an _Apple Distribution_ cert for 5U7E4UQ5M3. The Developer ID Application certs present belong to team FCSF68W94H and must not be used. Create a Developer ID Application cert for 5U7E4UQ5M3 and a `notarytool` credential before the first release.
- A Sparkle EdDSA key lives in CI secrets. Sparkle checks are the one network access (OQ-16).
- Both executables are signed. Install CLI is written from scratch; Zed's is GPL.
- As built (plan T5.1): cargo-packager lays out the `.app`; `scripts/package-release.sh` stamps `CFBundleVersion`, signs (ad-hoc without credentials) and makes the DMG with `hdiutil`, because cargo-packager's DMG step downloads `create-dmg` at build time and would package the app before it is signed.
- As built (plan T5.2): `scripts/sign-and-notarize.sh` signs the app inside out and then the DMG, notarizes with `notarytool` and staples, all from environment credentials. It refuses any identity that is not a Developer ID Application of team 5U7E4UQ5M3 (team FCSF68W94H by name) before touching a keychain, imports a CI certificate into a temporary keychain and restores the user's keychain search list on exit. Without credentials it skips and the artifacts stay ad-hoc signed. `.github/workflows/release.yml` runs it on `v*` tags and reads secrets only through step `env:`.
- As built (plan T5.4): the tap cask is rendered from `packaging/homebrew/polygloss.rb.tmpl` by `scripts/bump-tap.ts` on release tags. Install CLI links `/usr/local/bin/polygloss` to the bundle's `polygloss-cli`, replacing only symlinks; without write access it asks for an administrator password through `osascript`, with every path passed as an argument and shell-quoted by the script, never spliced into it.

## Alternatives rejected

| Option                            | Why not                                               |
| --------------------------------- | ----------------------------------------------------- |
| Mac App Store                     | The sandbox breaks git, sockets and the CLI install   |
| The official `homebrew/cask` repo | Requires a notable project; start with a personal tap |
| cargo-packager-updater            | Not published since 2025-07; no relaunch              |
| Signing with team FCSF68W94H      | Wrong team                                            |
