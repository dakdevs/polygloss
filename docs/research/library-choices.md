# Library choices (native GPUI stack)

Builders' reference for every third-party dependency of Polygloss v1. Everything here was checked live on **2026-09-28**. Versions come from the crates.io API and `npm view`. APIs were checked against the published crate sources, and compatibility was proven by compiling and running a throwaway workspace (see [Verification](#verification)).

- **Source of truth:** the design decision log (`polygloss-design-decisions.md` in the author's memory). If this doc and the log disagree, the log wins.
- **Out of scope:** web shells, Tauri/Electron, GPUiX/pierre-native, the Zed fork, GPL code, network access, and managed clones. All are rejected in the log.
- **Superseded log items not carried forward:** "vendored gpui_tokio" (the log now forbids vendoring Zed code) and "Zed test-support usable under GPL" (the license is now `MIT OR Apache-2.0`).
- **Crate layout:** follows [`docs/design.md` §24](../design.md#24-repository-layout) and the plan's ownership table: `polygloss-diff`, `-core`, `-highlight`, `-viewport`, `-platform` (no GPUI), `-app` (bin `Polygloss`), `-mcp`, `-cli` (bin `polygloss-cli`, on `PATH` as `polygloss`) and the dev-only `-perf`. The "Crate" column below uses those names without the `polygloss-` prefix. The verification workspace collapsed them into three members (core, app, cli). Features unify workspace-wide, so the result carries over.
- **Open-question numbers** `OQ-n` refer to `docs/design.md` §26. This doc adopts its provisional defaults wherever they touch libraries.

## At a glance

| Concern                              | Choice                                                                 | Version (2026-09-28)                                   | License                                                         | Crate               |
| ------------------------------------ | ---------------------------------------------------------------------- | ------------------------------------------------------ | --------------------------------------------------------------- | ------------------- |
| UI foundation                        | `gpui-kit` (→ `gpui-component`, `gpui-base`)                           | **=0.7.0**, which pins `gpui-pre =0.3.7` (zed@1a28cff) | Apache-2.0                                                      | app, viewport       |
| Async bridge                         | none: sync core, GPUI executors in the app, tokio only in the CLI      | —                                                      | —                                                               | —                   |
| Git (refs, renames, snapshots)       | system `git` CLI                                                       | ≥ 2.39 (provisional, OQ-6)                             | not linked (GPL stays out of the binary)                        | core                |
| Git (blob reads)                     | `gix` (`sha1` + `sha256`)                                              | 0.88.0                                                 | MIT OR Apache-2.0                                               | core                |
| Hunks and word diff                  | `gix-imara-diff`                                                       | 0.3.0                                                  | Apache-2.0                                                      | diff                |
| Syntax highlighting                  | `lumis` (tree-sitter 0.26.13)                                          | 0.15.0                                                 | MIT (plus each grammar's license)                               | highlight           |
| Comment markdown                     | `gpui_kit::base::TextView` (markdown-rs inside)                        | via gpui-kit; `markdown` 1.0.0 in core                 | Apache-2.0 / MIT                                                | app, core           |
| SQLite                               | `rusqlite` `bundled` (SQLite 3.53.2) + `rusqlite_migration`            | 0.40.2 / 2.6.0                                         | MIT / Apache-2.0                                                | core                |
| File watching                        | `notify` + `notify-debouncer-full`                                     | 8.2.0 / 0.7.0                                          | CC0-1.0 / MIT OR Apache-2.0                                     | app                 |
| IPC (app socket)                     | `interprocess` (sync local sockets)                                    | 2.4.4                                                  | 0BSD OR Apache-2.0                                              | core                |
| MCP server                           | `rmcp` (`server`, `macros`, `transport-io`) + `schemars`               | ~3.5.0 / 1.2.2                                         | Apache-2.0 / MIT                                                | mcp, cli            |
| Async runtime (CLI only)             | `tokio`                                                                | 1.53.1                                                 | MIT                                                             | mcp, cli            |
| JSON                                 | `serde` + `serde_json`                                                 | 1.0.229 / 1.0.151                                      | MIT OR Apache-2.0                                               | all                 |
| CLI                                  | `clap` (derive)                                                        | 4.6.7                                                  | MIT OR Apache-2.0                                               | cli                 |
| Hashing (diff IDs)                   | `sha2` + `hex`                                                         | 0.11.0 / 0.4.3                                         | MIT OR Apache-2.0                                               | core                |
| Fuzzy matching                       | `nucleo-matcher`                                                       | 0.3.1                                                  | MPL-2.0 (used unmodified)                                       | app                 |
| Logging                              | `tracing` + `tracing-subscriber` + `tracing-appender`                  | 0.1.44 / 0.3.23 / 0.2.5                                | MIT                                                             | all                 |
| Errors                               | `thiserror` (libraries), `anyhow` (binaries)                           | 2.0.21 / 1.0.104                                       | MIT OR Apache-2.0                                               | all                 |
| Tests                                | `insta`, `tempfile`, gpui-kit `test-support`, cargo-nextest            | 1.48.0 / 3.27.0 / 0.7.0 / 0.9.146                      | Apache-2.0 / MIT OR Apache-2.0 / Apache-2.0 / Apache-2.0 OR MIT | dev                 |
| Benchmarks                           | `criterion` (no plotters)                                              | 0.8.2                                                  | Apache-2.0 OR MIT                                               | dev                 |
| macOS gaps (Dock badge, Sparkle FFI) | `objc2`, `objc2-foundation`, `objc2-app-kit`                           | 0.6.4 / 0.3.2 / 0.3.2                                  | MIT / MIT / Zlib OR Apache-2.0 OR MIT                           | platform            |
| Packaging                            | `cargo-packager` (tool, not a dependency)                              | 0.11.8                                                 | Apache-2.0 OR MIT                                               | CI                  |
| Auto-update                          | Sparkle.framework through our own objc2 FFI                            | 2.10.0                                                 | MIT                                                             | platform            |
| TS black-box tests                   | `@modelcontextprotocol/client`, `@types/bun`, `typescript`, `prettier` | 2.2.0 / 1.4.2 / 7.0.2 / 3.9.9                          | MIT / MIT / Apache-2.0 / MIT                                    | root `package.json` |

## Verification

Throwaway workspace at `/tmp/polygloss-deps-check` (sources kept; `target/` deleted afterwards). It has three members, `polygloss-core`, `polygloss-app` and `polygloss-cli`, and uses the exact `[workspace.dependencies]` block [below](#verified-workspacedependencies). Each member calls the key API of every dependency.

| Check                                                                                   | Result                                                                                                                                                                                                                                                           |
| --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo +1.98.1 generate-lockfile`                                                       | Resolves: 1041 packages. No `links` conflict.                                                                                                                                                                                                                    |
| `cargo +1.98.1 check --workspace --all-targets`                                         | Passes. Includes dev-deps: gpui-kit `test-support`, criterion bench, insta. Re-run (30 s) after adding gix `sha256`, which is the final block.                                                                                                                   |
| `cargo test -p polygloss-core`                                                          | Passes: rusqlite WAL, a migration, `PRAGMA data_version`, imara Myers hunks with the indent heuristic, markdown-rs HTML-node detection, an insta inline snapshot, and **gix blob reads from both a sha1 and a sha256 repo** (`git init --object-format=sha256`). |
| `cargo test -p polygloss-app`                                                           | `#[gpui_kit::test]` entity test passes (headless GPUI).                                                                                                                                                                                                          |
| `bun test`: `@modelcontextprotocol/client` 2.2.0 spawns `polygloss mcp` (rmcp 3.5.0)    | `listTools` and `callTool` round-trip passes. `tsc` 7.0.2 typechecks with `@types/bun` 1.4.2. Prettier 3.9.9 check is clean.                                                                                                                                     |
| `cargo tree -e normal -p polygloss-cli`                                                 | 186 crates, **zero `gpui-*`**: the slim binary works.                                                                                                                                                                                                            |
| Network audit: `cargo tree -e normal,build -i {ureq,reqwest,wasmtime,curl,openssl-sys}` | None on macOS. `gpui-pre-reqwest` and `hyper`/`rustls` appear only under `cfg(target_family = "wasm")`.                                                                                                                                                          |
| `cargo build --release` (default profile, no LTO)                                       | GUI binary 69.5 MB (65.3 MB after `strip -x`). CLI binary 4.1 MB (3.1 MB). Warm-registry timings on this Mac: check 34 s, app test build 39 s, release 66 s. Peak `target/` 8.3 GB.                                                                              |

**Toolchain:** pin `rust-toolchain.toml` to **1.98.1**. That is Zed's `rust-toolchain.toml` at the gpui-pre snapshot rev `1a28cff`, and it is what we verified. Older compilers were not tested; earlier research reported ≥1.95 working. Declared MSRVs: `rusqlite_migration` 1.95, `lumis` 1.91, `gix*`/`rmcp` 1.88.

**Toolchain gotchas (hit during verification):**

- Homebrew `rustc`, `rustdoc` and `cargo` 1.93 come first on `PATH` on this machine and shadow rustup. Doc-tests failed with `E0514 … compiled by an incompatible version of rustc` until `RUSTDOC` was set as well. Fix: export `RUSTC=$(rustup which rustc)` and `RUSTDOC=$(rustup which rustdoc)`, or put `~/.cargo/bin` first on `PATH`.
- No Metal toolchain is needed. gpui-kit hard-enables `gpui_platform/runtime_shaders`, so shaders compile at runtime. We cannot turn this off because features are additive.
- There is a future-incompat warning for `block v0.1.6`, which comes via `cocoa` in `gpui-pre-macos`. It is upstream and harmless today.

## 1. UI foundation: gpui-kit

| Item                | Detail                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Crates on crates.io | `gpui-kit` 0.7.0 is the facade. It depends on `gpui-component` 0.7.0 (widgets), `gpui-base` 0.7.0 (rich text, root, input core), `gpui-kit-assets` 0.7.0 (icons), and `gpui-pre =0.3.7` plus `gpui-pre-platform =0.3.7` (`font-kit`, `runtime_shaders`). The lib name is `gpui_kit`. All are Apache-2.0. The repo is `longbridge/gpui-kit` (14.9k stars, pushed 2026-09-28). GitHub shows the license as NOASSERTION only because the repo has several license files. |
| gpui-pre            | "gpui-pre snapshot of zed@1a28cff" (`[package.metadata.gpui-pre] zed-rev = 1a28cff4…`). It is a Zed **main** snapshot, not a stable tag. The lib name is `gpui`.                                                                                                                                                                                                                                                                                                      |
| Cadence             | Tags v0.6.1 (09-09), v0.6.2/v0.6.4 (09-18), v0.6.6 (09-21), v0.7.0 (09-28). Expect roughly weekly breaking bumps, so pin `=0.7.0` and treat upgrades as a chore (the log accepts this).                                                                                                                                                                                                                                                                               |
| Features            | Default: `component`, `assets`. Use `test-support` **only** in `[dev-dependencies]`. Leave `inspector`, `profiler`, `decimal` and all `tree-sitter-*` **off** (see §4). **Webview:** gpui-kit has no webview feature. `gpui-wry` 0.7.0 is a separate crate. Never add it; the log rejects embedded web views.                                                                                                                                                         |
| Why                 | It is the only maintained native widget kit with Tree, VirtualList, multi-line Input, rich TextView, Command, Popover, Notification, resizable panes, native_menu, and headless UI test locators, all Apache-2.0. Decided in Q26.                                                                                                                                                                                                                                     |
| Key APIs            | `gpui_kit::application().run(\|cx\| { gpui_kit::init(cx); … })`; `gpui_kit::open_window(opts, cx, \|w, cx\| cx.new(…))`, which wraps the view in `base::Root`; `use gpui_kit::*` for all of GPUI; `gpui_kit::component::{tree, virtual_list, input, command, popover, notification, resizable, native_menu, …}`. Docs: <https://docs.rs/gpui-kit/0.7.0>, <https://gpui-kit.com>.                                                                                      |

**Gotchas**

- Don't add `gpui-pre` as a direct dependency. gpui-kit's facade decision (2026-09-08, `src/lib.rs`) says to reach GPUI via `gpui_kit::*`, and the `actions!` macro is re-exported for that reason. If a direct dependency is ever unavoidable, it must be `gpui = { package = "gpui-pre", version = "=0.3.7" }`, exactly matching gpui-kit's pin.
- GPUI's default HTTP client is `NullHttpClient`, so remote `img()`/URL loads fail closed. **Never call `cx.set_http_client`.** Leaving the null client in place is what keeps the app offline.
- Copy gpui-kit's own `[profile.dev.package]` `opt-level = 3` overrides for `gpui-pre*`, `taffy`, `resvg`, `rustybuzz`, `ttf-parser`, `smol`, `ropey`, `markdown` and `tree-sitter`. Without them, debug builds scroll poorly.
- `gpui-component` depends on `notify ^7`. We use notify 8, so there are two copies. Both are small and neither has a `links` key.
- `gpui_kit::init` lists every installed font family (`TextSystem::all_font_names`, a CoreText scan over XPC, ≈ 440 ms on the dev machine) whenever its theme's UI family is `.SystemUIFont` or its mono family is the platform default (`Menlo`), which is the case on a fresh theme. Polygloss initializes it through `polygloss_viewport::kit::init_kit`, which names both families first (plan T2.10.2).

## 2. Async model (the "tokio bridge")

**Decision for builders:** don't bridge.

| Layer                             | Runtime                                                                                                                                                                                                                                                    |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `polygloss-core`                  | **Synchronous**: `std::process` for git, `gix`, `rusqlite`, `interprocess` sync. It is runtime-agnostic and shared by both binaries.                                                                                                                       |
| `polygloss-app` (GPUI)            | Run core calls in `cx.background_spawn(async move { … })` (the `AppContext` trait) or `cx.background_executor().spawn(…)`. Use timers via `background_executor().timer(d)`. Hand results back with `cx.spawn` / `Entity::update`. **No tokio in the GUI.** |
| `polygloss-cli` (`polygloss mcp`) | `#[tokio::main(flavor = "current_thread")]`. Call core via `tokio::task::spawn_blocking`.                                                                                                                                                                  |

- `gpui_tokio` is Zed-internal. It is not on crates.io (`gpui_tokio` and `gpui-tokio` are both NOT FOUND), and vendoring it contradicts "written from scratch".
- **gpui-kit's own recommendation**, if a tokio-only library is ever needed in the GUI (`website/docs/image.md`, "reqwest needs a tokio runtime; GPUI's executors are not one"): use `static RUNTIME: LazyLock<tokio::runtime::Runtime>` built with `new_multi_thread().worker_threads(1).enable_all()`, then `RUNTIME.spawn(fut).await` inside a GPUI task. A tokio `JoinHandle` is an ordinary future. gpui-kit's `docs/gpui-shell.md` likewise states "This workspace does not depend on tokio".
- Rejected: vendoring `gpui_tokio` (Zed code); `async-compat` shims (hidden global runtime, same outcome with less control).

## 3. Git

### 3a. System git CLI (refs, merge-base, file list and renames, snapshots)

Run it through `std::process::Command` in core with these on every call:

- Pass `-C <worktree>`.
- Clear inherited `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY` and `GIT_COMMON_DIR`. Agents and hooks often set them.
- Set `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0` (or pass `--no-optional-locks`, so we never take `index.lock` from under the user) and `LC_ALL=C`.
- Stay offline with `GIT_NO_LAZY_FETCH=1`, an empty `GIT_ALLOW_PROTOCOL` and `-c protocol.allow=never`, per design.md §6.2 and §19. `GIT_NO_LAZY_FETCH` is ignored by gits before 2.44. `protocol.allow=never` alone does not cover them, because `protocol.<name>.allow` (user config or inherited `GIT_CONFIG_*`) and an inherited `GIT_ALLOW_PROTOCOL` override it; an empty `GIT_ALLOW_PROTOCOL` allows no transport whatever the config says (checked with git 2.50 and 2.54).
- Use `-z` everywhere, so paths are never quoted or escaped.

| Purpose                              | Command (verified on git 2.54.0)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Repo identity (worktrees collapse)   | `git rev-parse --path-format=absolute --git-common-dir --git-dir --show-toplevel`. Without `--path-format=absolute` (git ≥2.31) you get a relative `.git`.                                                                                                                                                                                                                                                                                                                                                                                                              |
| Object format (feeds the diff ID)    | `git rev-parse --show-object-format` → `sha1`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Resolve inputs                       | `git rev-parse --verify --end-of-options <rev>^{commit}` and `…^{tree}` (the diff ID hashes **tree** OIDs)                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Merge base                           | `git merge-base <a> <b>`. Exit code 1 means no common ancestor.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Default branch (offline)             | `git symbolic-ref -q refs/remotes/origin/HEAD`. Exit code 1 means unset, so fall back per OQ-5.                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Ref and commit pickers               | `git for-each-ref -z --format=… refs/heads refs/remotes refs/tags`; `git log -z -n200 --format=%H%x00%T%x00%P%x00%an%x00%at%x00%s`                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| File list and git-exact renames      | `git diff-tree -r -z --raw -M50% -l1000 --no-ext-diff --no-textconv --full-index <base_tree> <head_tree>`. Records are `:<mode> <mode> <oid> <oid> <status>\0<path>[\0<path>]`. Status is `R075` etc. Mode `160000` means a submodule. For merge commits, diff against the resolved first-parent tree.                                                                                                                                                                                                                                                                  |
| Live snapshot (user index untouched) | See design.md §5.1. Copy `<git-dir>/index` to a scratch dir outside the worktree (reuses the stat cache, so only changed files are rehashed). Then run `git add -A` and `git write-tree` with `GIT_INDEX_FILE=<scratch index>`, `GIT_OBJECT_DIRECTORY=<scratch>/objects` and `GIT_ALTERNATE_OBJECT_DIRECTORIES=<repo objects>`. To pin: copy the missing objects into the repo, then `git update-ref refs/polygloss/snapshots/<tree> <tree>`. Verified: `.gitignore` is honored, the user's staged set is unchanged, and scratch writes leave `.git/objects` untouched. |

**Gotchas**

- `git add -A` runs clean/smudge filters (LFS) and repo hooks' config in the user's repo, and writes loose objects. This is accepted by the log, but document it.
- Always pass `-M50% -l1000` (OQ-7: git's defaults, pinned). `diff-tree` otherwise honors the user's `diff.renameLimit`, which breaks determinism.
- A `.git` _file_ means a linked worktree. The index lives in `--git-dir`, not in `<toplevel>/.git`.

Rejected: libgit2/`git2` (C dependency, no git-exact renames), bundling git (GPL in our binary), `gix` rename tracking (the default rewrite limit dropped 71 of 173 renames on a Linux range, and even unlimited, 4 pairs differ).

### 3b. gix 0.88.0 (in-process blob reads)

| Item     | Detail                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Features | `default-features = false`, with `["sha1", "sha256", "max-performance-safe"]`. `max-performance-safe` adds pack LRU caches and `parallel`. `sha256` is needed for design.md's sha256 golden vectors; verified reading blobs from a `--object-format=sha256` repo. Add `"revision"` **only** for in-process `rev_parse*`/`merge_base`, which are `#[cfg(feature = "revision")]`. The verified block includes it; it is optional, because the CLI does revs. Never enable `blob-diff`: it pulls gix-filter, worktree and command.                                                                                        |
| Open     | `gix::open_opts(git_dir, gix::open::Options::isolated())` reads only repo-local config, with no env and no `git` binary spawn. By contrast, gix's `open_opts_with_git_binary_config` sets `git_binary = true`. Keep a `ThreadSafeRepository` and call `.to_thread_local()` per background task; `Repository` is not `Sync`. **As built (plan T1.4):** blob reads skip `open_opts` and use `gix::odb::Store::at_opts(<common_dir>/objects, kind, no replacements, …)` with `ignore_replacements` handles, because gix 0.88 applies `refs/replace/*` when repo config says `core.useReplaceRefs=false` (inverted sense). |
| Reads    | `repo.find_blob(oid)?.data`, `repo.find_tree`, `repo.find_commit`. For **scratch-store** states (design.md §5.1), open the scratch store directly with `gix::odb::at(<scratch>/objects, gix::hash::Kind::Sha1 or Sha256)` and use `gix::objs::FindExt::find_blob(&id, &mut buf)`. Put an `info/alternates` file in the scratch store that points at the repo's objects dir, so one handle reads both. Verified. Detect binary content yourself (git's rule: a NUL in the first 8000 bytes).                                                                                                                            |
| Docs     | <https://docs.rs/gix/0.88.0/gix/fn.open_opts.html>, <https://docs.rs/gix/0.88.0/gix/struct.Repository.html#method.find_blob>                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |

Gotchas:

- Some gix 0.88 errors are `gix_error::Exn<…>`, which does **not** implement `std::error::Error`, so `?` into `anyhow` or `Box<dyn Error>` fails. Use `.map_err(|e| e.into_error())`, which returns a `gix_error::Error` that implements `std::error::Error`.
- `gix::odb::at` takes the hash kind explicitly. Pass the value from `--show-object-format`.

Rejected: `git cat-file --batch` as the primary reader (a process per repo plus manual framing; fine as a fallback), `git2`.

### 3c. gix-imara-diff 0.3.0 (hunks and word diffs)

- **Use the crate directly.** Its only dependencies are `bstr` and `hashbrown`. `gix_diff::blob::diff_with_slider_heuristics` is literally `Diff::compute` followed by `postprocess_lines`, so there is no reason to pull `gix-diff/blob`. It is released in lockstep with `gix`; bump them together.
- API (<https://docs.rs/gix-imara-diff/0.3.0>):
  - `InternedInput::new(before, after)`. For raw blobs use `sources::byte_lines`, not the `&str` form, which requires UTF-8.
  - `Diff::compute(Algorithm::Myers | Algorithm::Histogram, &input)`.
  - `diff.postprocess_lines(&input)` applies `IndentHeuristic`. It is git's slider/indent heuristic, with tab width 8 as in xdiff.
  - `diff.hunks()` yields `Hunk { before: Range<u32>, after: Range<u32> }`.
  - Word diff per hunk: `hunk.latin_word_diff(&input, &mut word_input, &mut word_diff)`. Reuse the buffers and call `word_input.clear()` periodically.
  - Char diff: `Diff::compute_with(algo, &before_tokens, &after_tokens, num_tokens)`.
- Context and hunk merging are ours, since imara only yields changed ranges. Research measured GitHub parity with `-U3 --inter-hunk-context=1`, which means merging when the gap is 7 lines or fewer. The CI parity suite against `git diff --diff-algorithm=myers --indent-heuristic` (per the log) guards this.
- The `unified_diff` feature is only needed for parity-test output.
- **Gotcha (found by T1.16's parity repos):** imara's Myers runs git's `xdl_cleanup_records` step (drop lines absent from the other file, and frequent "multimatch" lines such as blank lines inside runs of unmatched ones) only _after_ trimming the common prefix and suffix, so its frequency limit (`bogosqrt`) and occurrence counts cover the trimmed middle instead of the whole files as in git. A blank line inside a rewritten block then splits one change in two. `polygloss-diff` therefore runs git's trim and cleanup itself on the whole files (`git_myers.rs`) and hands imara only the kept lines. imara then prunes lines of that reduced input that are absent from the other reduced side, which git does not; that still flips Myers tie-breaks on rare files (about 1 in 420 on some seeds).
- Rejected: `imara-diff` 0.2.0 (stale since 2025-06; gitoxide's fork carries the slider heuristic), `similar` (up to 30× slower on kernel diffs per imara's benchmarks), parsing `git diff` text with `diffy` (the log chose in-process hunks, independent of the user's git version and config).

## 4. Syntax highlighting: lumis 0.15.0

| Item       | Detail                                                                                                                                                                                                                                                                                                                                                                                                         |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Features   | `default-features = false` (the default is `all-languages`, 110+ grammars). Pick `lang-<name>` and bundles: `lang-bundle-web` (css, html, js, json, tsx, ts), `-backend` (c#, elixir, erlang, go, java, kotlin, php, protobuf, python, ruby, rust, scala, sql, ts…), `-system` (asm, bash, c, cmake, cpp, go, llvm, make, rust, wat, zig, zsh), `-web-extra`, `-full`. The verified set is in the block below. |
| No network | `lumis` depends on `lumis-wasm-runtime` with `default-features = false`. That crate's default `wasm` feature would add `wasmtime` plus the **`ureq` HTTP client**. Verified absent (`cargo tree -i ureq` is empty). Never enable it.                                                                                                                                                                           |
| API        | `lumis::languages::Language::guess(Some(ext_or_name), src)`; `lumis::highlight::highlight_iter_with_options(src, lang, None, HighlightOptions::new().budget(Budget::new().time_limit(Some(ms)).match_limit(n)).cancellation(&flag), \|text, lang, byte_range, scope, style\| …)`. `flag` is an `AtomicUsize`, non-zero cancels. Docs: <https://docs.rs/lumis/0.15.0/lumis/highlight/>.                         |
| Colours    | Pass `theme: None` and map the `scope` (nvim-treesitter capture names such as `keyword.function`) to our Zed-format theme's `syntax` map by longest prefix. `lumis::themes::from_json` exists if a lumis-native theme is ever wanted. See Open questions.                                                                                                                                                      |
| Offsets    | Ranges are UTF-8 **byte** ranges, which is also what GPUI `TextRun.len` uses. Split at line boundaries; no UTF-16 conversion is needed. That conversion was a pierre-native requirement.                                                                                                                                                                                                                       |

**The tree-sitter `links` conflict and its resolution**

- `tree-sitter` declares `links = "tree-sitter"`, so only one semver-compatible version can exist in the graph.
- Today `lumis` needs `^0.26.9` and `gpui-component` (optional, behind its `tree-sitter*` features) needs `^0.26.13`. Both resolve to **0.26.13**. This was verified both with and without gpui-kit's `tree-sitter-rust`/`tree-sitter-typescript` enabled.
- `tree-sitter` **0.27.0** shipped on 2026-08-30. A test manifest mixing `tree-sitter 0.27` with `gpui-component/tree-sitter` fails with _"package `tree-sitter` links to the native library `tree-sitter`, but it conflicts with a previous package"_.
- **Resolution:** keep **all gpui-kit `tree-sitter*` features off**, so lumis is the only tree-sitter user. The comment composer doesn't need syntax highlighting, and code blocks in comments use a lumis highlighter (§5). A future lumis→0.27 bump then cannot conflict. If a gpui-kit tree-sitter feature is ever enabled, bump lumis and gpui-kit in the same PR.

Gotchas:

- Each `highlight_iter*` call builds a fresh highlighter and parses the whole input. Highlight whole old and new blobs on the background executor, cache by blob OID (blobs are immutable), and use `Budget` for 200k-line files.
- Grammar licenses (mostly MIT) must be aggregated into the app's third-party notices.

Rejected: gpui-kit's built-in tree-sitter highlighter (it would pull a second grammar set and re-create the `links` coupling), `arborium` (tree-sitter fork, `links` clash), `giallo` (EUPL-1.2, Shiki-speed class), `syntect` (TextMate regexes, slower and less accurate), Zed's language crates (GPL, and they pin a tree-sitter git rev).

## 5. Comment markdown and sanitization

- **What gpui-kit uses internally:** `gpui-base` parses markdown with **`markdown` 1.0.0** (markdown-rs, `to_mdast`, GFM and math on) and parses raw HTML with `html5ever` 0.27 plus `markup5ever_rcdom`. `pulldown-cmark`, `comrak` and `ammonia` are **not** in the graph.
- **Use `gpui_kit::base::TextView`, not the `gpui_kit::component::text::TextView` compat facade.** The facade lacks `code_block_highlighter` and `image_source` (this was a verified compile error).
- Key APIs (`gpui-base` 0.7.0, `src/text/text_view.rs`):
  - `TextView::markdown(id, src)`
  - `.code_block_highlighter(|block| lumis_ranges(block.code(), block.lang()))`
  - `.image_source(|uri| …)`
  - `.on_link_click(|url, ev, win, cx| …)`
  - `.plugin(impl MarkdownPlugin)`
  - `.markdown_block_parser(…)` / `.markdown_block_renderer(…)`
- Set the highlighter **per view**. `Theme::sync_base` (gpui-component `theme/mod.rs`) re-installs the global `TextViewDefaults` on every theme change and would drop a globally installed highlighter.
- **Sanitization, with no extra crate**, for agent-authored bodies:
  1. A `MarkdownPlugin` that claims `mdast::Node::Html` (block and inline) and renders it as literal text.
  2. `image_source` returns a bundled placeholder for anything that isn't `data:` or a bundled asset. Remote loads already fail under `NullHttpClient`; this is belt and braces.
  3. `on_link_click` opens only `https:`, `http:` and `mailto:` via `cx.open_url`. The **default opens any URL**, including `file:` and custom schemes.
- ` ```suggestion ` blocks: in the app, a block parser claims `Node::Code { lang: Some("suggestion") }` and renders a mini-diff. In core (MCP structured output), parse the same way with `markdown::to_mdast(body, &ParseOptions::gfm())` using the same crate and version, so both sides agree.
- Rejected: `pulldown-cmark` 0.13.4 or `comrak` 0.55.0 with our own renderer (a second parser and a renderer to build), `ammonia` 4.2.0 (it sanitizes HTML strings, but we never render HTML).

## 6. SQLite: rusqlite 0.40.2 (`bundled`) + rusqlite_migration 2.6.0

| Item     | Detail                                                                                                                                                                                                                                                                                                                                                                          |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Why      | The fastest and most mature binding. It is sync (fits the core model), and `bundled` pins **SQLite 3.53.2** (`libsqlite3-sys` 0.38.2, `links = "sqlite3"`), so the app and MCP use identical SQLite. `rusqlite_migration` uses `user_version`.                                                                                                                                  |
| APIs     | `Connection::open`; `pragma_update(None, "journal_mode", "WAL")`; `busy_timeout(Duration)`; `set_transaction_behavior(TransactionBehavior::Immediate)` on writers; `pragma_query_value(None, "data_version", \|r\| r.get(0))`; `Migrations::from_slice(&[M::up(…)]).to_latest(&mut conn)`. Docs: <https://docs.rs/rusqlite/0.40.2>, <https://docs.rs/rusqlite_migration/2.6.0>. |
| Gotchas  | See the list below.                                                                                                                                                                                                                                                                                                                                                             |
| Rejected | `sqlx` 0.9 (async, and its SQLite migrate lock is a no-op), Zed `sqlez` (GPL), `diesel` (heavy for ~10 tables), `honker` (alpha).                                                                                                                                                                                                                                               |

Gotchas:

- `data_version` changes only for commits made by _other_ connections. Poll it on a dedicated read-only connection that never writes.
- Run migrations while holding `std::fs::File::lock()` (stable since Rust 1.89) on a sidecar lock file, so the app and `polygloss mcp` can't race.
- Retry the WAL switch on `SQLITE_BUSY` at startup.
- Set `foreign_keys=ON` per connection.
- Never add a second SQLite crate. Only one `links = "sqlite3"` may exist.

## 7. File watching: notify 8.2.0 + notify-debouncer-full 0.7.0

- API: `new_debouncer_opt::<_, RecommendedWatcher, NoCache>(Duration::from_millis(200), None, tx, NoCache, notify::Config::default())`, then `.watch(path, RecursiveMode::Recursive)`. `tx` can be a `std::sync::mpsc::Sender` or a closure; forward events into a GPUI task. The FSEvents backend (`macos_fsevent`) is on by default. Docs: <https://docs.rs/notify-debouncer-full/0.7.0>.
- **Use `NoCache`.** On macOS, `RecommendedCache` is `FileIdMap`, which walks the entire tree on `watch()`. That is expensive on Linux-size repos, and we re-derive changes via git anyway.
- Watch the worktree root **plus** the absolute `--git-dir` and `--git-common-dir`, which are outside the tree for linked worktrees.
- Drop `.git/objects/**` and `*.lock` noise. Batch-filter ignored paths with `git check-ignore -z --stdin` before recomputing.
- Treat `need_rescan` or directory-level FSEvents as "something changed". Use design §10's debounce (200 ms trailing, 1 s max wait): the < 500 ms watcher→banner budget is measured from a single save, so the trailing delay plus recompute must fit in 500 ms.
- Rejected: notify 9.0.0-rc.5 (pre-release), `notify-debouncer-mini` 0.7.0 (fine, but less event dedup), raw `fsevent-sys` (not portable), watchman (external daemon).

## 8. IPC: interprocess 2.4.4 (app ⇄ CLI/MCP)

- No features: use the sync API. The server is `ListenerOptions::new().name(path.to_fs_name::<GenericFilePath>()?).create_sync()?`; the client is `Stream::connect(name)`. Speak newline-delimited JSON via `serde_json`. Docs: <https://docs.rs/interprocess/2.4.4/interprocess/local_socket/>.
- In the app, run the accept loop on a dedicated `std::thread` and forward over a `futures::channel::mpsc` into a GPUI foreground task. In MCP, use the sync client inside `spawn_blocking`.
- Gotchas:
  - macOS `sun_path` is limited to **104 bytes**. `~/Library/Application Support/polygloss/polygloss.sock` fits typical home paths; if it would not fit, fall back to `$TMPDIR/polygloss-<uid>/polygloss.sock` in a `0700` directory (design §13.2).
  - Create the directory with mode 0700 before binding.
  - Handle a stale socket after a crash: probe with `connect` first, and only then pass `.try_overwrite(true)`. The same probe doubles as the single-instance check.
- Rejected: `tokio::net::UnixListener` (puts tokio in the GUI and is not Windows-portable, while the log requires portability), `ipc-channel` (heavier, bincode), XPC or distributed notifications (macOS-only).

## 9. MCP: rmcp ~3.5.0 + schemars 1.2.2

| Item             | Detail                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pin              | Use `~3.5.0` (patch updates only). Releases are weekly: 3.0.0 on 07-28, 3.4.0 on 09-15, 3.4.1 on 09-23, **3.5.0 today**, which bumps protocol `LATEST`. Minor bumps are deliberate PRs.                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Features         | `default-features = false, features = ["server", "macros", "transport-io"]`. `server` implies `schemars` and `transport-async-rw`. That adds no HTTP, reqwest or axum.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Key APIs         | `#[tool_router] impl S { #[tool(description = "…")] async fn f(&self, Parameters(a): Parameters<T>) -> Result<CallToolResult, ErrorData> }`; `#[tool_handler(router = self.tool_router)] impl ServerHandler for S { fn get_info(&self) -> ServerConfig { ServerConfig::new(ServerCapabilities::builder().enable_tools().build()) } }`; `S::new().serve(rmcp::transport::stdio()).await?.waiting().await?`; `CallToolResult::success(vec![ContentBlock::text(…)])`. Client name (e.g. `claude-code`): `ctx.peer.peer_info()` → `InitializeRequestParams.client_info.name`. Long-poll cancellation: `RequestContext.ct`. Docs: <https://docs.rs/rmcp/3.5.0>. |
| Verified gotchas | See the list below.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Rejected         | `StreamableHttpService`/axum (adds a network listener, which we don't need), a TS MCP server under Bun (second runtime), community crates (`mcp-sdk`, etc.).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |

Verified gotchas:

- `model::Content` is now **`ContentBlock`**.
- `ServerInfo` is deprecated; use **`ServerConfig`**.
- `#[tool_handler]` without `router = …` rebuilds `Self::tool_router()` on every request.
- The default `serverInfo` is `rmcp`/`3.5.0`. Set a name and version, either in `get_info` or with `#[tool_handler(name = "polygloss", version = …)]`.
- **stdout is the JSON-RPC channel.** Log to stderr (or a file) with `with_ansi(false)`, never `println!`. rmcp logs at INFO when the stream ends, so default the filter to `warn`.

## 10. Small crates

| Concern    | Choice                                                                                 | Notes                                                                                                                                                                                                                                                                                                                                              | Rejected (one line)                                                         |
| ---------- | -------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| JSON       | `serde` 1.0.229 (`derive`), `serde_json` 1.0.151                                       | Settings, keymap, themes, IPC and MCP payloads. Strict JSON (see Open questions on JSONC).                                                                                                                                                                                                                                                         | `simd-json` (no need)                                                       |
| CLI        | `clap` 4.6.7 (`derive`)                                                                | `polygloss [show\|compare\|open\|mcp\|wait]`; `--json` output mode.                                                                                                                                                                                                                                                                                | `argh`/`lexopt` (weaker help and completions)                               |
| Hashing    | `sha2` 0.11.0, `hex` 0.4.3                                                             | `Sha256::new()`, `.update()`, `hex::encode(h.finalize())`. The 0.11 output is a `hybrid_array::Array`, so use `hex::encode`, not `{:x}`. Diff ID encoding per OQ-1: `sha256("polygloss/diff/v1\n" + objfmt + "\n" + base_tree + "\n" + head_tree)`, lowercase hex. Lock it with golden vectors (insta) for sha1 and sha256 repos.                  | `blake3` (no benefit for ID-sized input; TS parity is simpler with SHA-256) |
| Fuzzy      | `nucleo-matcher` 0.3.1                                                                 | `Matcher::new(Config::DEFAULT.match_paths())`; `Pattern::parse(q, CaseMatching::Smart, Normalization::Smart).match_list(items, &mut m)`. Run it inside a gpui-kit list delegate's search, because the built-in Command filter is substring-only. It is MPL-2.0 (file-level copyleft): use it unmodified and ship its notice. Last release 2024-02. | `frizbee` 0.13.0 (MIT, active, SIMD): the fallback if nucleo stalls         |
| Logging    | `tracing` 0.1.44, `tracing-subscriber` 0.3.23 (`env-filter`), `tracing-appender` 0.2.5 | The app writes a rolling file under `~/Library/Logs/polygloss/`. The CLI and MCP write to stderr only. The default `tracing-log` feature bridges GPUI's `log` records.                                                                                                                                                                             | `env_logger` (no spans), `log` alone                                        |
| Errors     | `thiserror` 2.0.21 in core, `anyhow` 1.0.104 in binaries                               | Core exposes typed errors so MCP can map them to `ErrorData`.                                                                                                                                                                                                                                                                                      | `eyre`/`snafu` (no gain)                                                    |
| Async glue | `futures` 0.3.34                                                                       | `channel::mpsc` from the socket thread or watcher to GPUI. Already in the graph via GPUI and rmcp.                                                                                                                                                                                                                                                 | `async-channel` (also fine; one fewer name)                                 |

## 11. Testing and benchmarks

| Tool                                                                  | Use                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| --------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo-nextest` 0.9.146                                               | A binary tool, not a dependency. Install a prebuilt binary (see <https://nexte.st>) in CI and locally; it is not installed on this Mac yet. Process-per-test isolation suits GPUI globals. `bun run test:unit` runs `cargo nextest run --workspace`.                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `insta` 1.48.0 (`json`) + `cargo-insta` 1.48.0                        | Snapshots of the diff model, row layout, MCP payloads and diff IDs.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `tempfile` 3.27.0                                                     | Temporary repos and databases.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| gpui-kit `test-support` (dev-dep)                                     | `#[gpui_kit::test] fn t(cx: &mut TestAppContext)`, including the async form. `gpui_kit::test::{TestWindowExt, TestAppContextExt}` provides `find`/`within`/`click`/`right_click`/`drag_to`/`scroll`/`press`/`input`/`wait_for`. Its `ElementSnapshot` does **not** inspect pixels. For screenshot baselines use gpui-pre's `HeadlessAppContext::capture_screenshot(window) -> RgbaImage` (built with `gpui_kit::platform::current_platform(true).text_system()` and `gpui_kit::platform::current_headless_renderer`, on the main thread; `VisualTestAppContext` opens real off-screen windows whose scale follows the display) with our own clean-room baseline runner (plan T2.8). |
| `criterion` 0.8.2 (`default-features = false`, `cargo_bench_support`) | Micro-benchmarks for hunks, word diff, highlighting and ID hashing. Frame-time budgets are measured in-app; gpui-kit's `profiler` feature is available for perf runs only.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |

Gotcha: with `test-support` on, `use gpui_kit::*;` also imports GPUI's `test` attribute, which shadows `#[test]`. Import test types explicitly (gpui-kit `tests/test_macro.rs`).

Rejected: plain `cargo test` as the runner (shared-process GPUI globals), Zed's `visual_test_runner` (GPL), `divan` 0.1.21 (no release since 2025-04), `iai-callgrind` (no valgrind on macOS arm64).

## 12. macOS integration: what gpui-pre 0.3.7 already provides

| Need                   | gpui-pre 0.3.7                                                                                                                                                                                                  | Plan                                                                                                                                             |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `polygloss://` URLs    | `Application::on_open_urls(FnMut(Vec<String>))` (`application:openURLs:`); register it **before** `run()`.                                                                                                      | Use it. Declare `CFBundleURLTypes` via cargo-packager `deep-link-protocols`. `App::register_url_scheme` (runtime) is not needed.                 |
| Reopen / Dock click    | `Application::on_reopen`                                                                                                                                                                                        | Reopen the window after the last one was closed; the app keeps running, per the log.                                                             |
| System notifications   | `App::show_system_notification(SystemNotification { tag, title, body, actions })`, `App::on_system_notification_response`, `App::set_app_identity`. Built on UNUserNotificationCenter; the auth prompt is lazy. | Use it; **no objc2 needed**. It aborts outside an app bundle ("not-in-a-bundle abort"), so gate it on a bundle identifier in `cargo run` builds. |
| Dock badge             | **None** (no `dockTile` in `gpui-pre-macos`)                                                                                                                                                                    | objc2 (verified compile): `NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref())`. Needs `MainThreadMarker`.          |
| Menus                  | `App::set_menus`, `App::set_dock_menu`, gpui-kit `native_menu`                                                                                                                                                  | Use them.                                                                                                                                        |
| Restart                | `App::set_restart_path`, `App::restart`                                                                                                                                                                         | Sparkle relaunches itself; keep these for "relaunch to apply".                                                                                   |
| Hidden launch from MCP | n/a                                                                                                                                                                                                             | `open -g -b dev.dak.polygloss [polygloss://…]`                                                                                                   |

The objc2 family is already in the graph through gpui-component: `objc2` 0.6.4, `objc2-foundation` 0.3.2, `objc2-app-kit` 0.3.2 (whose default features include `NSApplication` and `NSDockTile`). `objc2` 0.5.2 also appears, via `accesskit_macos`. Don't use it.

## 13. Packaging: cargo-packager 0.11.8

- Status: last release 2025-11-27, but the repo is active (pushed 2026-09-28). Install with `cargo install cargo-packager --locked` in CI.
- Config lives under `[package.metadata.packager]` in `polygloss-app/Cargo.toml` or in `Packager.toml`. Kebab-case keys are accepted.

```toml
[package.metadata.packager]
product-name = "Polygloss"
identifier = "dev.dak.polygloss"
formats = ["app", "dmg"]
before-packaging-command = "cargo build --release -p polygloss-app -p polygloss-cli"
binaries = [{ path = "Polygloss", main = true }, { path = "polygloss-cli" }] # both land in Contents/MacOS
icons = ["packaging/icon.icns"]
deep-link-protocols = [{ schemes = ["polygloss"] }] # → CFBundleURLTypes

[package.metadata.packager.macos]
minimum-system-version = "14.0"                  # provisional, OQ-18
signing-identity = "Developer ID Application: … (5U7E4UQ5M3)"
entitlements = "packaging/entitlements.plist"    # hardened runtime; no exceptions expected
info-plist-path = "packaging/Info.plist"         # merged last: its keys override generated ones
frameworks = ["vendor/Sparkle.framework"]
```

- Secrets are env-only; they cannot be set in the config:
  - Signing: `APPLE_CERTIFICATE` (base64 .p12) and `APPLE_CERTIFICATE_PASSWORD`.
  - Notarization, any one of: `APPLE_API_KEY` + `APPLE_API_ISSUER` + `APPLE_API_KEY_PATH` (preferred for CI); `APPLE_ID` + `APPLE_PASSWORD` + `APPLE_TEAM_ID`; or `APPLE_KEYCHAIN_PROFILE`.
- Codesign runs with `--options runtime --timestamp`, signing every Mach-O file in the bundle inside-out. Then it runs `notarytool` and `stapler staple`.

Gotchas:

- Binary names must differ by more than case. The default APFS volume is case-insensitive, so `Polygloss` and `polygloss` cannot share `Contents/MacOS` (design.md §13.1). The Homebrew cask exposes `polygloss-cli` as `polygloss`.
- The generated `CFBundleVersion` is a UTC timestamp (`YYYYMMDD.HHMMSS`). Sparkle compares against it, so set `CFBundleVersion` explicitly in `info-plist-path`.
- The log notes that a _Developer ID Application_ cert for team 5U7E4UQ5M3 and a notarytool credential don't exist yet.

Rejected: `cargo-bundle` 0.12.0 (actively released: 0.10 in 2026-04, 0.12 on 2026-09-20; per the research verifier we would still script `notarytool` and stapling ourselves), Zed's bundle scripts (GPL), the Tauri bundler (Tauri is rejected), `cargo-packager-updater` (see §14).

## 14. Auto-update: Sparkle 2.10.0 via our own objc2 FFI

1. **Vendor the framework:** a script fetches the official `Sparkle-2.10.0` release into `vendor/` (gitignored, checksum-pinned). Delete `Versions/B/XPCServices/`, since the XPC services are only for sandboxed apps (Sparkle docs, "Removing the XPC Services"), and ship it via cargo-packager `macos.frameworks`.
2. **Load at runtime, no link-time dependency:**
   - `NSBundle` → `…/Contents/Frameworks/Sparkle.framework` → `load()`.
   - `AnyClass::get(c"SPUStandardUpdaterController")` → `msg_send![alloc, initWithStartingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil]`.
   - Retain the controller for the app's lifetime. The "Check for Updates…" menu item sends `checkForUpdates:`.
   - Dev and test builds without the bundle simply skip the updater.
   - Run all of this on the main thread (`MainThreadMarker`).
3. **Info.plist:** `SUFeedURL` (appcast on GitHub Releases or Pages) and `SUPublicEDKey` (from Sparkle's `bin/generate_keys`).
4. **CI:** `bin/generate_appcast` / `sign_update` with the EdDSA private key as a secret. The appcast's `sparkle:version` must equal `CFBundleVersion`.
5. **Signing:** Sparkle's docs sign `Versions/B/Autoupdate` and `Versions/B/Updater.app` with `codesign -f -s "$ID" -o runtime`, then the framework. cargo-packager signs Mach-O files, but whether it re-signs the nested `Updater.app` bundle wrapper is **unverified**. Check with `codesign --verify --deep --strict` and `spctl -a -vv` in the packaging spike.

Rejected: `sparkle-updater` 0.1.0 (MIT, published 2026-09-19, 212 downloads), `gpui-updater-pre` 0.1.0 (young, has its own updater model), `cargo-packager-updater` 0.2.3 (no release since 2025-07; no EdDSA appcast or delta UX), Homebrew-only updates (no in-app update).

## 15. TypeScript black-box test side (root `package.json`)

| Package                        | Version | Notes                                                                                                                                                                                                            |
| ------------------------------ | ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `@modelcontextprotocol/client` | 2.2.0   | `import { Client } from "@modelcontextprotocol/client"` and `import { StdioClientTransport } from "@modelcontextprotocol/client/stdio"`. Deps include `zod ^4.2`. Verified against `polygloss mcp` (rmcp 3.5.0). |
| `@types/bun`                   | 1.4.2   | Local Bun is 1.3.14. `bun-types` 1.4.2 is the same content.                                                                                                                                                      |
| `typescript`                   | 7.0.2   | npm `latest` is the native compiler. `tsc -p .` works with `"module": "Preserve", "moduleResolution": "bundler", "types": ["bun"]`.                                                                              |
| `prettier`                     | 3.9.9   | Formats TS, JSON and Markdown (`bun run format` pairs it with `cargo fmt`).                                                                                                                                      |

Gotcha: `bun init` writes `typescript` as a **peerDependency** (`^5`), and a later `bun add -d typescript@7.0.2` silently left 5.9.3 installed. Delete the peerDependency first.

Rejected: `@modelcontextprotocol/sdk` 1.31.0 (the v1 monolith; v2 splits client and server), Vitest/Jest (the log mandates `bun test`).

## Compatibility and conflicts

| Issue                                           | Status                                                                                                        | Resolution                                                                                                                    |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `tree-sitter` `links` (lumis vs gpui-component) | Resolves today on 0.26.13; a 0.27 skew breaks (reproduced)                                                    | gpui-kit `tree-sitter*` features stay off (§4)                                                                                |
| `sqlite3` `links`                               | Single `libsqlite3-sys` 0.38.2                                                                                | rusqlite is the only SQLite crate                                                                                             |
| gpui-kit exact pin `gpui-pre =0.3.7`            | OK                                                                                                            | Never depend on `gpui-pre` directly; pin `gpui-kit =0.7.0`                                                                    |
| Duplicates (macOS target)                       | `notify` 7+8, `objc2` 0.5+0.6, `objc2-app-kit` 0.2+0.3, `thiserror` 1+2, `syn` 2+3, `darling`, `itertools`, … | Accepted; all small, none with `links`. Revisit notify when gpui-kit moves to 8.                                              |
| `sha2` 0.10 + 0.11                              | 0.10 only via `oo7` → `gpui-pre-linux`                                                                        | Not built on macOS                                                                                                            |
| Network-capable crates                          | None in the macOS normal graph                                                                                | CI check: `cargo tree -e normal,build -i <crate>` must fail for `ureq`, `reqwest`, `hyper`, `wasmtime`, `curl`, `openssl-sys` |
| `rmcp` weekly releases                          | 3.5.0 published today                                                                                         | `~3.5.0`; bump the minor deliberately with the TS E2E suite                                                                   |
| MSRV                                            | `rusqlite_migration` 1.95 is the highest declared                                                             | Toolchain 1.98.1                                                                                                              |

## Verified `[workspace.dependencies]`

This is exactly the block that passed `check --workspace --all-targets` and the core and app tests on rustc 1.98.1. The release-size build ran just before gix `sha256` was added. Member usage is in the table after the block. The lumis language list is provisional.

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.98.1"
profile = "minimal"
components = ["rustfmt", "clippy"]
```

```toml
[workspace]
resolver = "3"
members = ["crates/polygloss-core", "crates/polygloss-app", "crates/polygloss-cli"] # verification layout; real layout: design.md §24

[workspace.package]
edition = "2024"
rust-version = "1.98.1"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
# --- UI: app, viewport, highlight, platform ---
gpui-kit = { version = "=0.7.0", default-features = false, features = ["component", "assets"] }
lumis = { version = "=0.15.0", default-features = false, features = [
  "lang-bundle-web", "lang-bundle-backend", "lang-bundle-system",
  "lang-markdown", "lang-markdown-inline", "lang-yaml", "lang-toml", "lang-diff",
  "lang-dockerfile", "lang-swift", "lang-objc", "lang-lua", "lang-nix", "lang-graphql",
  "lang-scss", "lang-vue", "lang-svelte", "lang-xml", "lang-hcl"
] }
nucleo-matcher = "0.3.1"
notify = "8.2.0"
notify-debouncer-full = "0.7.0"
objc2 = "0.6.4"
objc2-foundation = { version = "0.3.2", features = ["NSString", "NSBundle"] }
objc2-app-kit = { version = "0.3.2", features = ["NSApplication", "NSDockTile"] }

# --- core, diff (no GPUI; shared by app and cli) ---
gix = { version = "0.88.0", default-features = false, features = ["sha1", "sha256", "revision", "max-performance-safe"] }
gix-imara-diff = "0.3.0"
rusqlite = { version = "0.40.2", features = ["bundled"] }
rusqlite_migration = "2.6.0"
markdown = "1.0.0"
sha2 = "0.11.0"
hex = "0.4.3"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
interprocess = "2.4.4"
thiserror = "2.0.21"
anyhow = "1.0.104"
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter"] }
tracing-appender = "0.2.5"
futures = "0.3.34"

# --- mcp, cli (no GPUI) ---
clap = { version = "4.6.7", features = ["derive"] }
rmcp = { version = "~3.5.0", default-features = false, features = ["server", "macros", "transport-io"] }
schemars = "1.2.2"
tokio = { version = "1.53.1", features = ["rt", "macros", "io-std", "io-util", "sync", "time"] }

# --- dev / test / bench ---
insta = { version = "1.48.0", features = ["json"] }
tempfile = "3.27.0"
criterion = { version = "0.8.2", default-features = false, features = ["cargo_bench_support"] }

[profile.dev.package]
gpui-pre = { opt-level = 3 }
gpui-pre-platform = { opt-level = 3 }
gpui-pre-sum-tree = { opt-level = 3 }
tree-sitter = { opt-level = 3 }
gix-imara-diff = { opt-level = 3 }
```

Member wiring. The verification collapsed these into core, app and cli; this is the mapping onto design.md §24:

| Crate                 | Dependencies                                                                                                                                                                                                                                                                                                                                                                         |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `polygloss-diff`      | gix-imara-diff, serde, thiserror. Dev: insta, criterion. The leaf crate: no git, IO or GPUI.                                                                                                                                                                                                                                                                                         |
| `polygloss-core`      | diff, gix, rusqlite, rusqlite_migration, markdown, sha2, hex, serde, serde_json, interprocess, thiserror, tracing, uuid, libc (plan OQ-P2, OQ-P8). Dev: insta, tempfile.                                                                                                                                                                                                             |
| `polygloss-highlight` | lumis, serde, serde_json (Zed-format theme model), thiserror. Dev: insta, criterion.                                                                                                                                                                                                                                                                                                 |
| `polygloss-viewport`  | diff, highlight, gpui-kit. No core dependency (the app adapts core data to the `DiffProvider` trait). Dev: gpui-kit `test-support`.                                                                                                                                                                                                                                                  |
| `polygloss-platform`  | No GPUI. polygloss-core (`DataPaths`), and libc (`geteuid`), sha2 and hex for the launcher's socket fallback (T4.2; these three go when it delegates to core's `ipc::socket_path`). Behind feature `appkit` and `[target.'cfg(target_os = "macos")'.dependencies]`: objc2, objc2-foundation, objc2-app-kit (Dock badge, Sparkle). The CLI uses it without `appkit` for its launcher. |
| `polygloss-app`       | core, diff, highlight, viewport, platform (`appkit`), gpui-kit, nucleo-matcher, notify, notify-debouncer-full, futures, anyhow, tracing-subscriber, tracing-appender, regex. Dev: `gpui-kit` with `test-support`, insta, image.                                                                                                                                                      |
| `polygloss-mcp`       | core, platform (launcher), rmcp, schemars, serde, serde_json, tokio (only for `spawn_blocking`).                                                                                                                                                                                                                                                                                     |
| `polygloss-cli`       | core, mcp, platform (launcher), clap, tokio, anyhow, tracing-subscriber. No GPUI, lumis or tree-sitter (`scripts/check-deps.sh`).                                                                                                                                                                                                                                                    |
| `polygloss-perf`      | core, diff, highlight, viewport, gpui-kit, anyhow, futures, serde_json, tempfile, libc (plan T2.9 As built, OQ-P8). Dev: core `test-support`. Dev-only, never shipped.                                                                                                                                                                                                               |

Notifications (`App::show_system_notification`) and URL handling (`Application::on_open_urls`) are GPUI APIs, so the app calls them directly; `polygloss-platform` does not need gpui-kit.

## Open questions

Each question has a provisional default so building can start. This doc follows design.md's provisional defaults for the library-relevant ones below; they are not re-decided here:

- OQ-1: diff-ID encoding
- OQ-5: default-branch fallback
- OQ-6: git ≥ 2.39, which is above the 2.31 needed for `--path-format`
- OQ-7: `-M50% -l1000`
- OQ-16: Sparkle is the single egress
- OQ-17: no tokio in the GUI
- OQ-18: macOS 14

| #   | Question                                                                                                                                 | Provisional default                                                                                                                                                                                                    |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| L1  | **Needs the user's call.** Sparkle vs the log's "the app never touches the network". The log mandates both.                              | Per OQ-16: Sparkle is the only egress. Sparkle's standard opt-in prompt (`SUEnableAutomaticChecks` unset) and a setting to turn it off.                                                                                |
| L2  | **Needs the user's call** (design OQ-29). Binary layout: the log says both "single binary" and "`polygloss mcp` = separate slim binary". | Per design.md §13.1: `Polygloss` (GPUI) plus `polygloss-cli` (CLI + `mcp` + `wait`; on `PATH` as `polygloss`). Verified that the slim graph has zero `gpui-*` crates and strips to 3.1 MB.                             |
| L3  | Syntax colours: lumis themes or our Zed-format theme?                                                                                    | Scopes from lumis. Colours come from the active Zed-format theme's `syntax` map (Pierre Light/Dark by default), matched by longest scope prefix, so any loaded Zed theme also recolours code.                          |
| L4  | lumis language set (binary size vs coverage).                                                                                            | The verified list in the block. Measure grammar size in the release binary before adding `lang-bundle-web-extra`/`-full`.                                                                                              |
| L5  | Settings and keymap files: strict JSON or JSONC (Zed-style comments)?                                                                    | Strict JSON via `serde_json`. Zed theme files are plain JSON, so themes are unaffected.                                                                                                                                |
| L6  | Does cargo-packager correctly re-sign Sparkle's nested `Updater.app` and `Autoupdate`?                                                   | Verify in the packaging spike (`codesign --verify --deep --strict`, `spctl -a -vv`, notarization log). Fallback: a post-sign script running Sparkle's documented `codesign` sequence, then `notarytool` and `stapler`. |
| L7  | Bench harness: criterion or divan?                                                                                                       | criterion 0.8.2 (maintained, with baselines). divan has had no release since 2025-04.                                                                                                                                  |
| L8  | notify 8 (ours) plus notify 7 (via gpui-component): accept the duplicate or align on 7?                                                  | Stay on 8.2 (current stable, matching debouncer 0.7). Revisit when gpui-kit bumps.                                                                                                                                     |
| L9  | Keep gix `revision` now that the CLI owns revs and merge-base (ADR-0014)?                                                                | Keep it (verified, small). Drop it at v1 feature freeze if nothing calls `rev_parse*`/`merge_base`.                                                                                                                    |
| L10 | Toolchain policy across gpui-kit upgrades.                                                                                               | Track Zed's `rust-toolchain.toml` at the `zed-rev` recorded in the pinned `gpui-pre` (`[package.metadata.gpui-pre]`). Today that is 1.98.1.                                                                            |

## Sources (live, 2026-09-28)

- crates.io API `https://crates.io/api/v1/crates/<name>[/<ver>/dependencies]` and published crate sources for gpui-kit, gpui-component, gpui-base, gpui-pre, gpui-pre-macos, gpui-pre-platform, lumis, lumis-core, lumis-wasm-runtime, gix, gix-diff, gix-imara-diff, rmcp, rmcp-macros, interprocess, notify-debouncer-full, nucleo-matcher and cargo-packager.
- GitHub: `longbridge/gpui-kit` (workspace `Cargo.toml`, `website/docs/image.md`, `docs/gpui-shell.md`, tags), Zed `rust-toolchain.toml` at rev `1a28cff`, `sparkle-project/Sparkle` releases, `crabnebula-dev/cargo-packager` releases.
- Sparkle docs: <https://sparkle-project.org/documentation/> (setup keys, manual `codesign` sequence) and `/documentation/sandboxing/` (XPC services only for sandboxed apps).
- npm: `npm view` for `@modelcontextprotocol/{client,sdk,server}`, `@types/bun`, `bun-types`, `typescript`, `prettier`.
- Prior research: `/tmp/polygloss-research-result.json` (pure_rust_gpui recommendations, critique) and `/tmp/polygloss-zed-feasibility.json` (toolchain and Metal gotchas, perf lessons).
