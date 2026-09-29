#!/usr/bin/env bash
# Dependency audit for the workspace's "must not contain" rules (docs/plan.md,
# "Workspace layout and crate ownership"). Exits non-zero on any violation.
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

# 1. The slim CLI links no GPUI, lumis or tree-sitter.
forbid polygloss-cli '^(gpui|lumis|tree-sitter)' "slim CLI: no gpui, lumis or tree-sitter"

# 2. Only the viewport, app and perf crates may reach GPUI.
for crate in polygloss-diff polygloss-core polygloss-highlight polygloss-platform polygloss-mcp; do
  forbid "$crate" '^gpui' "no GPUI outside viewport, app and perf"
done

# 3. diff and core stay runtime- and highlighter-free.
for crate in polygloss-diff polygloss-core; do
  forbid "$crate" '^(lumis|tokio|rmcp)' "no lumis, tokio or rmcp in diff and core"
done

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
