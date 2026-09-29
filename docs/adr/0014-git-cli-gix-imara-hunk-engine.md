# ADR-0014: git CLI for structure, gix for blobs, imara for hunks

- **Status:** Accepted. Supersedes the research pipeline built on git ≥ 2.50 `diff-pairs`.
- **Date:** 2026-09-28
- **Design:** [§6 Git and diff engine](../design.md#6-git-and-diff-engine)

## Context

Research benchmarked the Linux v6.10..v6.11 range (13,283 paths):

- `git diff-tree -r -z --raw -M` took 171 ms and paired renames exactly like git.
- gix rename tracking dropped 71 of 173 renames with its default limit, ran about 40× slower with no limit, and still paired 4 renames differently.
- libgit2 found 181 renames, disagreeing with git on 10.

Research's pipeline took patches from `git diff-pairs`, which needs git ≥ 2.50. The Xcode Command Line Tools ship 2.39.5, so that would have meant bundling git.

## Decision

- The **git CLI** handles refs, merge-base, the file list with git-exact renames (`diff-tree -r -z --raw -M`) and snapshots (temp index plus `write-tree`). **Any recent git version** works; there is no ≥ 2.50 requirement.
- **gix** reads blobs in process.
- **gix-imara-diff** computes hunks and word diffs lazily, for files near the viewport, on background threads.
- The default algorithm is **Myers plus the indent/slider heuristic**, which is GitHub-like. Histogram is optional.
- Hunks do not depend on the user's git version or config.
- A **CI parity suite** compares our hunks with `git diff` on real repos.

## Consequences

- No patch text to parse. One engine does hunks, word ranges, carry-forward mapping and open-in-editor mapping.
- A few files may diverge from git or GitHub hunks. The parity suite tracks this, and anchors are blob lines, so comments are unaffected.
- Rename pairing can vary with the git version, so `file_changes` is stored per `diff_id`.
- There is a minimum system git version check (Provisional 2.39, OQ-6). Git flags and environment are pinned (design §6.2).

## Alternatives rejected

| Option                            | Why not                                                    |
| --------------------------------- | ---------------------------------------------------------- |
| git ≥ 2.50 `diff-pairs` patches   | Requires bundling git (GPL) or a new git; patch parsing    |
| All-gix (tree diff with rewrites) | Rename parity and speed problems                           |
| libgit2 / git2-rs                 | C dependency, rename divergences, unstable SHA-256 support |
| Standalone imara-diff 0.2         | Stale; gitoxide maintains the fork we use                  |
