#!/usr/bin/env bash
# Dependency audit for the workspace's "Must not contain" rules (docs/plan.md,
# "Workspace layout and crate ownership"), one copy of each `links` crate, and no
# network-capable crates. Exits 1 on a violation, 2 if cargo cannot read the workspace.
#
#   scripts/check-deps.sh [--manifest-path <Cargo.toml>]
#
# Inspects the macOS arm64 normal+build graph (`cargo tree -e normal,build`).
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cargo="$script_dir/cargo.sh"
target="aarch64-apple-darwin"

manifest_args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest-path)
      manifest_args=(--manifest-path "$2")
      shift 2
      ;;
    *)
      echo "check-deps: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

failures=0
fail() {
  echo "check-deps: FAIL: $*" >&2
  failures=$((failures + 1))
}

tree() {
  "$cargo" tree ${manifest_args[@]+"${manifest_args[@]}"} -e normal,build --target "$target" "$@"
}

# Unique package names reachable from a workspace member (the member included).
reachable() {
  local out
  if ! out="$(tree -p "$1" --prefix none --format '{p}')"; then
    echo "check-deps: cargo tree failed for $1" >&2
    exit 2
  fi
  printf '%s\n' "$out" | awk 'NF { print $1 }' | sort -u
}

# forbid <crate> <extended regex> <rule description>
forbid() {
  local crate="$1" pattern="$2" rule="$3" names name
  names="$(reachable "$crate")" || exit 2
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    fail "$crate reaches forbidden crate $name ($rule)"
  done <<<"$(printf '%s\n' "$names" | grep -E "$pattern" || true)"
}

# 1-3. The "Must not contain" column of the ownership table. Only the viewport,
# app and perf crates may reach GPUI; the slim CLI also links no tree-sitter.
# Package-name patterns (extended regex):
gpui='^gpui'
lumis='^lumis'
tree_sitter='^tree-sitter'
tokio='^tokio$'
rmcp='^rmcp'
git='^(gix|git2|libgit2-sys)$' # not gix-imara-diff, a standalone diff algorithm
sqlite='^(rusqlite|libsqlite3-sys)$'

forbid polygloss-diff "$gpui|$lumis|$tokio|$rmcp|$git|$sqlite" "polygloss-diff: no GPUI, lumis, tokio, rmcp, git or SQLite"
forbid polygloss-core "$gpui|$lumis|$tokio|$rmcp" "polygloss-core: no GPUI, lumis, tokio or rmcp"
forbid polygloss-highlight "$gpui|$git|$sqlite" "polygloss-highlight: no GPUI, git or SQLite"
forbid polygloss-viewport "$git|$sqlite" "polygloss-viewport: no git or SQLite"
forbid polygloss-platform "$gpui|$tokio" "polygloss-platform: no GPUI or tokio"
forbid polygloss-app "$tokio|$rmcp" "polygloss-app: no tokio or rmcp"
forbid polygloss-mcp "$gpui|$lumis" "polygloss-mcp: no GPUI or lumis"
forbid polygloss-cli "$gpui|$lumis|$tree_sitter" "slim CLI: no GPUI, lumis or tree-sitter"

# 4. One copy of each crate with a `links` key we depend on.
if ! dups="$(tree --workspace -d --depth 0 --prefix none --format '{p}')"; then
  echo "check-deps: cargo tree -d failed" >&2
  exit 2
fi
for name in tree-sitter libsqlite3-sys; do
  versions="$(printf '%s\n' "$dups" | awk -v n="$name" '$1 == n { print $2 }' | sort -u)"
  count="$(printf '%s' "$versions" | grep -c . || true)"
  if [ "$count" -gt 1 ]; then
    fail "more than one $name in the graph: $(echo "$versions" | tr '\n' ' ')"
  fi
done

# 5. Offline: no network-capable crates anywhere in the normal+build graph.
for name in ureq reqwest hyper curl wasmtime openssl-sys; do
  err_file="$(mktemp)"
  if out="$(tree --workspace -i "$name" --prefix none --format '{p}' 2>"$err_file")"; then
    if [ -n "$(printf '%s' "$out" | tr -d '[:space:]')" ]; then
      fail "network-capable crate $name is in the graph: $(printf '%s\n' "$out" | awk 'NF { print $1 }' | sort -u | tr '\n' ' ')"
    fi
  elif ! grep -q "did not match any packages" "$err_file"; then
    cat "$err_file" >&2
    rm -f "$err_file"
    echo "check-deps: cargo tree -i $name failed" >&2
    exit 2
  fi
  rm -f "$err_file"
done

if [ "$failures" -gt 0 ]; then
  echo "check-deps: $failures violation(s)" >&2
  exit 1
fi
echo "check-deps: ok"
