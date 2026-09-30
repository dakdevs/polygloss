#!/usr/bin/env bash
# Run rustup's cargo (never Homebrew's 1.93, which shadows it on PATH and breaks
# doc-tests with E0514) with a disk-friendly directory layout for git worktrees:
#
#   CARGO_BUILD_BUILD_DIR = <main checkout>/target-shared  intermediates, shared by all worktrees
#   CARGO_TARGET_DIR      = <this checkout>/target         final artifacts, one per worktree
#
# Both are derived from the checkout that contains this script, not the caller's
# cwd. Caller-provided CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR, RUSTC and RUSTDOC win.
#
# Sharing one build dir is only safe for registry dependencies. Cargo names
# workspace-member outputs by the package path relative to the workspace root, so
# every checkout writes the same files and cargo's mtime check would happily reuse
# another checkout's compiled members (and run its test binaries). So every command
# that can touch the build dir:
#
#   - holds an exclusive lock, <build dir>.lock, for the whole invocation (including
#     nextest's run phase and `cargo run`), so checkouts never interleave; and
#   - when the build dir was last used by a different checkout (recorded in
#     <build dir>/.polygloss-checkout), first runs `cargo clean --workspace` for every
#     profile dir present, so this checkout's members are rebuilt from its sources.
#
# `scripts/cargo.sh with-lock <command> [args…]` runs <command> (not cargo) under
# that lock, after the same claim; cargo.sh calls inside it from this checkout
# reuse the lock. Use it when a sequence of cargo calls must see each other's
# artifacts (scripts/package-release.sh: another checkout claiming the build dir
# between two builds cleans the first one's binaries from this target dir).
set -euo pipefail

cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
export PATH="$cargo_bin:$PATH"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
script_path="$script_dir/$(basename "${BASH_SOURCE[0]}")"

die() {
  echo "cargo.sh: $*" >&2
  exit 2
}

# Inherited git env (agents and hooks export these) must not redirect discovery.
repo_git() {
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR -u GIT_INDEX_FILE \
    git -C "$script_dir" "$@"
}

# True when the script sits inside a git checkout (a .git dir or file above it).
inside_git_checkout() {
  local dir="$script_dir"
  while :; do
    [ -e "$dir/.git" ] && return 0
    [ "$dir" = "/" ] && return 1
    dir="$(dirname "$dir")"
  done
}

git_err="$(mktemp)"
trap 'rm -f "$git_err"' EXIT
if toplevel="$(repo_git rev-parse --show-toplevel 2>"$git_err")" &&
  common_dir="$(repo_git rev-parse --path-format=absolute --git-common-dir 2>"$git_err")"; then
  main_checkout="$(cd "$common_dir/.." && pwd -P)"
elif inside_git_checkout; then
  # Falling back here would silently create another multi-GB target dir.
  cat "$git_err" >&2
  die "git cannot read the checkout containing $script_path; fix git (e.g. safe.directory) and retry"
else
  # Not a git checkout (e.g. a source tarball): one checkout, both dirs inside it.
  toplevel="$(cd "$script_dir/.." && pwd -P)"
  main_checkout="$toplevel"
fi
rm -f "$git_err"
trap - EXIT

: "${CARGO_TARGET_DIR:=$toplevel/target}"
: "${CARGO_BUILD_BUILD_DIR:=$main_checkout/target-shared}"
: "${RUSTC:=$cargo_bin/rustc}"
: "${RUSTDOC:=$cargo_bin/rustdoc}"
export CARGO_TARGET_DIR CARGO_BUILD_BUILD_DIR RUSTC RUSTDOC

run_cargo() {
  if [ "${1-}" = with-lock ]; then
    shift
    [ "$#" -gt 0 ] || die "with-lock needs a command"
    exec "$@"
  fi
  exec "$cargo_bin/cargo" "$@"
}

# Subcommands that never read or write the build dir skip the lock.
case "${1-}" in
  "" | -V | --version | version | -h | --help | help | --list | tree | metadata | \
    fmt | deny | locate-project | pkgid | verify-project | read-manifest | \
    generate-lockfile | update | search)
    run_cargo "$@"
    ;;
esac

build_dir="${CARGO_BUILD_BUILD_DIR%/}"
lock_file="$build_dir.lock"
owner_file="$build_dir/.polygloss-checkout"

current_owner() {
  if [ -f "$owner_file" ]; then cat "$owner_file"; fi
}

# Called with the lock held: make the build dir this checkout's before cargo runs.
claim_build_dir() {
  [ "$(current_owner)" = "$toplevel" ] && return 0
  if [ -d "$build_dir" ] && [ -f "$toplevel/Cargo.toml" ]; then
    local fingerprint dir rel profile err
    local -a target_args
    for fingerprint in "$build_dir"/*/.fingerprint "$build_dir"/*/*/.fingerprint; do
      [ -d "$fingerprint" ] || continue
      dir="$(dirname "$fingerprint")"
      rel="${dir#"$build_dir"/}"
      target_args=()
      case "$rel" in
        */*)
          profile="${rel#*/}"
          target_args=(--target "${rel%%/*}")
          ;;
        *) profile="$rel" ;;
      esac
      [ "$profile" = debug ] && profile=dev
      if ! err="$("$cargo_bin/cargo" clean -q --workspace --manifest-path "$toplevel/Cargo.toml" \
        --profile "$profile" ${target_args[@]+"${target_args[@]}"} 2>&1 >&2)"; then
        # A profile only another checkout defines cannot be built here, so it cannot go stale.
        case "$err" in
          *"profile \`$profile\` is not defined"*) continue ;;
        esac
        printf '%s\n' "$err" >&2
        die "could not clean this checkout's workspace members from $build_dir"
      fi
    done
  fi
  mkdir -p "$build_dir"
  printf '%s\n' "$toplevel" >"$owner_file"
}

if [ "${POLYGLOSS_CARGO_LOCK_HELD-}" = "$lock_file" ]; then
  if [ "${POLYGLOSS_CARGO_LOCK_ACQUIRED-}" = 1 ]; then
    # First run under the lock that this script just took.
    unset POLYGLOSS_CARGO_LOCK_ACQUIRED
    claim_build_dir
  elif [ "$(current_owner)" != "$toplevel" ]; then
    # Nested inside another cargo.sh call that holds this build dir's lock.
    die "a cargo.sh from another checkout ($(current_owner)) holds $build_dir; run this outside it"
  fi
  run_cargo "$@"
fi

mkdir -p "$(dirname "$lock_file")"
if ! /usr/bin/lockf -k -t 0 "$lock_file" /usr/bin/true 2>/dev/null; then
  echo "cargo.sh: waiting for another cargo.sh using $build_dir" >&2
fi
exec /usr/bin/lockf -k "$lock_file" /usr/bin/env \
  POLYGLOSS_CARGO_LOCK_HELD="$lock_file" POLYGLOSS_CARGO_LOCK_ACQUIRED=1 \
  "$script_path" "$@"
