#!/usr/bin/env bash
# Run rustup's cargo (never Homebrew's 1.93, which shadows it on PATH and breaks
# doc-tests with E0514) with a disk-friendly directory layout for git worktrees:
#
#   CARGO_BUILD_BUILD_DIR = <main checkout>/target-shared  intermediates, shared by all worktrees
#   CARGO_TARGET_DIR      = <this checkout>/target         final artifacts, one per worktree
#
# Both are derived from the checkout that contains this script, not the caller's
# cwd. Caller-provided CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR, RUSTC and RUSTDOC win.
set -euo pipefail

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
export PATH="$cargo_bin:$PATH"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

# Inherited git env (agents and hooks export these) must not redirect discovery.
repo_git() {
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR -u GIT_INDEX_FILE \
    git -C "$script_dir" "$@" 2>/dev/null
}

if toplevel="$(repo_git rev-parse --show-toplevel)" &&
  common_dir="$(repo_git rev-parse --path-format=absolute --git-common-dir)"; then
  main_checkout="$(cd "$common_dir/.." && pwd -P)"
else
  # Not a git checkout (e.g. a source tarball): one checkout, both dirs inside it.
  toplevel="$(cd "$script_dir/.." && pwd -P)"
  main_checkout="$toplevel"
fi

: "${CARGO_TARGET_DIR:=$toplevel/target}"
: "${CARGO_BUILD_BUILD_DIR:=$main_checkout/target-shared}"
: "${RUSTC:=$cargo_bin/rustc}"
: "${RUSTDOC:=$cargo_bin/rustdoc}"
export CARGO_TARGET_DIR CARGO_BUILD_BUILD_DIR RUSTC RUSTDOC

exec "$cargo_bin/cargo" "$@"
