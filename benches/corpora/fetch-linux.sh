#!/usr/bin/env bash
# The Linux corpus (plan T2.1, design §12.2, OQ-P7): kernel tags v6.10 and v6.11
# in one shallow repo, compared --direct. This script uses the network; the app
# and the CLI never do.
#
#   benches/corpora/fetch-linux.sh [--dry-run]
#
# The repo is $POLYGLOSS_LINUX_REPO, else $POLYGLOSS_CORPORA/linux, else
# polygloss-corpora/linux next to the main checkout: the same path
# `bun benches/corpora/manifest.ts --corpus linux` prints (lib.ts).
#
# - Both tags already resolve there: reuse it as is. Nothing is fetched or
#   written, so an existing full clone is safe to point at.
# - The path is missing, empty, or a repo with no refs yet (an interrupted
#   run): `git init`, then `git fetch --depth=1 <url> tag v6.10 tag v6.11`
#   from $POLYGLOSS_LINUX_URL (default: the GitHub mirror). Nothing is checked
#   out: the corpus is compared by tree, and a checkout would cost ~1.5 GB.
# - Anything else: refuse (exit 1) rather than modify someone else's repo.
#
# Prints the manifest entry { name, repo, base, head, mode } as one JSON line.
# --dry-run prints it for the resolved path without reading or writing it.
set -euo pipefail

prog="fetch-linux"
url="${POLYGLOSS_LINUX_URL:-https://github.com/torvalds/linux.git}"

die() {
  printf '%s: %s\n' "$prog" "$*" >&2
  exit 1
}

usage() {
  printf 'usage: benches/corpora/fetch-linux.sh [--dry-run]\n' >&2
  exit 2
}

dry_run=0
while (($#)); do
  case "$1" in
    --dry-run) dry_run=1 ;;
    *)
      printf '%s: unexpected argument %s\n' "$prog" "$1" >&2
      usage
      ;;
  esac
  shift
done

# Inherited variables that would point git at another repository.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR \
  GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE GIT_CEILING_DIRECTORIES \
  GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT
export LC_ALL=C GIT_TERMINAL_PROMPT=0

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
checkout="$(cd "$here/../.." && pwd -P)"

absolute() {
  local p="$1"
  [[ "$p" == /* ]] || p="$PWD/$p"
  while [[ "$p" == */ && "$p" != / ]]; do p="${p%/}"; done
  printf '%s' "$p"
}

corpora_root() {
  if [[ -n "${POLYGLOSS_CORPORA:-}" ]]; then
    absolute "$POLYGLOSS_CORPORA"
    return
  fi
  local main="$checkout"
  # Inside a git checkout a failing git is an error, never a second location.
  if [[ -e "$checkout/.git" ]]; then
    local common
    common="$(git -C "$checkout" rev-parse --path-format=absolute --git-common-dir)" ||
      die "git rev-parse --git-common-dir failed in $checkout"
    main="$(dirname "$common")"
  fi
  printf '%s/polygloss-corpora' "$(dirname "$main")"
}

if [[ -n "${POLYGLOSS_LINUX_REPO:-}" ]]; then
  repo="$(absolute "$POLYGLOSS_LINUX_REPO")"
else
  repo="$(corpora_root)/linux"
fi
[[ "$repo" =~ [[:cntrl:]] ]] && die "control characters in the repo path"

print_entry() {
  local s="$repo"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  printf '{"name":"linux","repo":"%s","base":"v6.10","head":"v6.11","mode":"direct"}\n' "$s"
}

if ((dry_run)); then
  print_entry
  exit 0
fi

# git in $repo itself: discovery never climbs into a parent repository, and
# read-only commands take no optional locks.
in_repo() {
  GIT_CEILING_DIRECTORIES="$(dirname "$repo")" GIT_OPTIONAL_LOCKS=0 git -C "$repo" "$@"
}

has_tags() {
  in_repo rev-parse --verify --quiet 'v6.10^{commit}' >/dev/null 2>&1 &&
    in_repo rev-parse --verify --quiet 'v6.11^{commit}' >/dev/null 2>&1
}

if [[ -d "$repo" ]] && has_tags; then
  print_entry
  exit 0
fi

if [[ -e "$repo" ]]; then
  [[ -d "$repo" ]] || die "$repo exists and is not a directory"
  if [[ -n "$(ls -A "$repo")" ]]; then
    refs="$(in_repo for-each-ref --count=1 2>/dev/null)" ||
      die "$repo exists and is not a git repo with v6.10 and v6.11; refusing to modify it (set POLYGLOSS_LINUX_REPO to another path, or remove it)"
    [[ -z "$refs" ]] ||
      die "$repo is a git repo without v6.10 and v6.11; refusing to modify it (set POLYGLOSS_LINUX_REPO to another path, or remove it)"
  fi
fi

mkdir -p "$repo"
printf '%s: fetching v6.10 and v6.11 (depth 1) from %s into %s\n' "$prog" "$url" "$repo" >&2
in_repo init --quiet --initial-branch=main
in_repo fetch --depth=1 --no-tags --quiet "$url" tag v6.10 tag v6.11
has_tags || die "fetched from $url but v6.10 and v6.11 do not resolve to commits in $repo"
print_entry
