# ADR-0005: Fully offline, local-only

- **Status:** Accepted. Supersedes the `github.com/owner/repo#N` review key and all GitHub API use.
- **Date:** 2026-09-28
- **Design:** [§3 Diff sources](../design.md#3-diff-sources-and-resolution), [§19 Security](../design.md#19-security-and-privacy)

## Context

The first design keyed PR reviews as `github.com/owner/repo#N` and loaded them through the GitHub API. Research found hard limits there. `/pulls/N/files` stops at 3,000 files. The diff media type returns HTTP 406 above 300 files. Compare diffs get cut off silently. The user then chose a local-only product.

## Decision

The app **never touches the network**:

- no `git fetch`
- no GitHub or GitLab API
- no managed clones
- no telemetry

It works only on repos that are already on disk.

A "PR" is a branch compare between two refs that already exist locally. The user or agent runs `gh pr checkout` or `git fetch` themselves, and can attach a display label such as "PR #123". The default branch is detected offline from `refs/remotes/origin/HEAD`.

Git runs with `GIT_NO_LAZY_FETCH=1`, `-c protocol.allow=never` and `GIT_TERMINAL_PROMPT=0`, so partial clones fail instead of fetching.

## Consequences

- No auth, no rate limits and no API caps.
- No PR titles or descriptions unless a label is given.
- Review keys are ref pairs (ADR-0007).
- A blobless or partial clone that is missing objects shows an error.
- CI runs an egress audit.
- Open exception: Sparkle update checks (design OQ-16).

## Alternatives rejected

| Option                                    | Why not                                  |
| ----------------------------------------- | ---------------------------------------- |
| GitHub API for metadata, git for content  | Network access, auth, and API limits     |
| Managed shallow or blobless clones        | Network access and disk management       |
| Read-only import of GitHub review threads | Network access. Possible post-v1 opt-in. |
