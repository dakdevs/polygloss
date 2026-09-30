#!/usr/bin/env bash
# Prepares the manual agent wake-up gate (plan "Manual gate", T4.12): builds
# Polygloss and polygloss-cli, creates a fresh gate root with a sandbox data dir
# whose stable CLI link (`<data>/bin/polygloss`, design §13.2) points at the
# built CLI, creates two scratch repos with one uncommitted change each, and
# prints the exact commands the human types next. The procedure and the results
# table are in docs/testing/agent-wake-gate.md.
#
# Never touches the real data dir, ~/.config or Claude Code's config: it runs
# no `claude` command, and it only deletes a previous gate root that carries
# its marker file and that no process still uses.
set -euo pipefail

readonly default_root=/tmp/polygloss-wake-gate
readonly marker=.polygloss-wake-gate
readonly steps=6

usage() {
  cat <<EOF
usage: scripts/wake-gate/prepare.sh [--dry-run] [--root <dir>] [--no-build]

Prepares the agent wake-up gate (docs/testing/agent-wake-gate.md).

  --dry-run      print every step and the commands to type; build and write nothing
  --root <dir>   gate root (default $default_root): data/, repo-a/, repo-b/;
                 a previous gate root there is replaced
  --no-build     use the binaries already in the target dir
  -h, --help     show this help
EOF
}

die() {
  printf 'prepare.sh: %s\n' "$*" >&2
  exit 1
}

dry_run=0
build=1
root=$default_root
while [ $# -gt 0 ]; do
  case "$1" in
  --dry-run) dry_run=1 ;;
  --no-build) build=0 ;;
  --root)
    [ $# -ge 2 ] || {
      printf 'prepare.sh: --root needs a directory\n' >&2
      exit 2
    }
    root=$2
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    printf 'prepare.sh: unknown option %s\n' "$1" >&2
    usage >&2
    exit 2
    ;;
  esac
  shift
done

[ "$(uname -s)" = Darwin ] || die "the wake gate runs on macOS only"

# Absolute paths, resolved against the caller's directory.
case "$root" in /*) ;; *) root="$PWD/$root" ;; esac
root="${root%/}"
[ -n "$root" ] || root=/

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cd "$repo_root"
target_dir="${CARGO_TARGET_DIR:-target}"
case "$target_dir" in /*) ;; *) target_dir="$repo_root/$target_dir" ;; esac
target_dir="${target_dir%/}"
app_bin="$target_dir/debug/Polygloss"
cli_bin="$target_dir/debug/polygloss-cli"
data_dir="$root/data"
repo_a="$root/repo-a"
repo_b="$root/repo-b"

# Never the real data dir, config or Claude Code state, nor a whole home or /.
home="${HOME:-}"
case "$root" in
/ | "$home") die "refusing gate root $root: pick a dedicated directory" ;;
esac
if [ -n "$home" ]; then
  for real in "$home/Library" "$home/.config" "$home/.claude"; do
    case "$root/" in
    "$real"/*) die "refusing gate root $root: it is inside $real (real app or Claude Code state)" ;;
    esac
  done
fi

# What to do with an existing root: replace a previous gate root that nothing
# uses, reuse an empty directory, refuse anything else.
replace=0
if [ -e "$root" ] || [ -L "$root" ]; then
  [ -d "$root" ] && [ ! -L "$root" ] || die "$root exists and is not a directory"
  if [ -f "$root/$marker" ]; then
    real_root="$(cd "$root" && pwd -P)"
    users="$(lsof -t +D "$real_root" 2>/dev/null | sort -un | tr '\n' ' ' || true)"
    if [ -n "${users// /}" ]; then
      die "$root is still in use by pid(s) ${users% }; quit Polygloss and Claude Code (and cd out of it) first"
    fi
    replace=1
  elif [ -n "$(ls -A "$root")" ]; then
    die "$root is not empty and is not a wake-gate root (no $marker); pick another --root"
  fi
fi

q() { printf '%q' "$1"; }

step_no=0
step() {
  step_no=$((step_no + 1))
  printf 'step %d/%d: %s\n' "$step_no" "$steps" "$*"
}

# One commit and one uncommitted change: `x` for W1's rename prompt.
make_scratch_repo() {
  local repo=$1
  local g=(git -C "$repo" -c core.hooksPath=/dev/null -c commit.gpgsign=false
    -c user.name="Polygloss Wake Gate" -c user.email=wake-gate@polygloss.invalid)
  mkdir -p "$repo/src"
  git -C "$repo" init -q -b main
  # `--scope local` plugin settings must never show up in the review.
  printf '.claude/\n' >>"$repo/.git/info/exclude"
  cat >"$repo/README.md" <<'EOF'
# Wake gate scratch repo

Throwaway repo for the Polygloss agent wake-up gate.
EOF
  cat >"$repo/src/scale.js" <<'EOF'
// Scales a value by a factor.
export function scale(x, factor) {
  return x * factor;
}
EOF
  "${g[@]}" add README.md src/scale.js
  "${g[@]}" commit -q -m "Add scale"
  cat >>"$repo/src/scale.js" <<'EOF'

// Moves a value by an offset.
export function shift(x, offset) {
  return x + offset;
}
EOF
}

if [ "$dry_run" = 1 ]; then
  printf 'dry run: nothing is built or written\n'
fi
printf 'Polygloss wake-up gate in %s (checkout %s)\n\n' "$root" "$repo_root"

# 1-2: build, one -p per build (a joint build would unify features and link
# polygloss-platform's `appkit` into the CLI, plan T5.1).
if [ "$build" = 1 ]; then
  step "build the app: scripts/cargo.sh build -p polygloss-app"
  [ "$dry_run" = 1 ] || scripts/cargo.sh build -p polygloss-app
  step "build the CLI: scripts/cargo.sh build -p polygloss-cli"
  [ "$dry_run" = 1 ] || scripts/cargo.sh build -p polygloss-cli
else
  step "skip building the app (--no-build): use $app_bin"
  step "skip building the CLI (--no-build): use $cli_bin"
fi
if [ "$dry_run" = 0 ]; then
  for bin in "$app_bin" "$cli_bin"; do
    [ -x "$bin" ] || die "$bin is missing or not executable; build it (drop --no-build)"
  done
fi

# 3: a fresh gate root.
if [ "$replace" = 1 ]; then
  step "replace the previous gate root $root (sandbox data dir $data_dir, 0700)"
else
  step "create the gate root $root with the sandbox data dir $data_dir (0700)"
fi
if [ "$dry_run" = 0 ]; then
  [ "$replace" = 0 ] || rm -rf -- "$root"
  mkdir -p -- "$root"
  : >"$root/$marker"
  mkdir -m 700 -- "$data_dir"
fi

# 4: the stable CLI path the plugin's shim tries first (design §16.1). The dev
# app never repoints it (only a bundled app does, T4.3).
step "link $data_dir/bin/polygloss -> $cli_bin"
if [ "$dry_run" = 0 ]; then
  mkdir -m 700 -- "$data_dir/bin"
  ln -s -- "$cli_bin" "$data_dir/bin/polygloss"
fi

# 5-6: scratch repos.
step "create scratch repo A $repo_a (one commit, one uncommitted change to src/scale.js)"
[ "$dry_run" = 1 ] || make_scratch_repo "$repo_a"
step "create scratch repo B $repo_b (the same, for W7)"
[ "$dry_run" = 1 ] || make_scratch_repo "$repo_b"

if [ "$dry_run" = 0 ]; then
  printf '\nready: %s\n' "$root"
fi

cat <<EOF

Now, by hand (docs/testing/agent-wake-gate.md has every case, W1-W8):

1. Record the versions in the results table:

   claude --version
   sw_vers -productVersion

2. In a fresh terminal (W1-W7). The env is inherited by the MCP server, the
   Stop hook and the dev app it launches. POLYGLOSS_TEST=1 also enables the
   test-only debug_state socket op and the hidden \`polygloss debug\` commands
   in that session; export it together with POLYGLOSS_APP_BIN, never alone.

   export POLYGLOSS_DATA_DIR=$(q "$data_dir")
   export POLYGLOSS_TEST=1
   export POLYGLOSS_APP_BIN=$(q "$app_bin")
   export PATH="\$POLYGLOSS_DATA_DIR/bin:\$PATH"
   cd $(q "$repo_a")
   claude plugin marketplace add $(q "$repo_root") --scope local
   claude plugin install polygloss@polygloss --scope local
   claude

   W3 only: before \`claude\`, also run
   export POLYGLOSS_WAIT_TIMEOUT_S=120

   W7: a second terminal with the same exports, then
   cd $(q "$repo_b")
   claude plugin marketplace add $(q "$repo_root") --scope local
   claude plugin install polygloss@polygloss --scope local
   claude

3. Checks while Claude is idle (any terminal with the exports):

   polygloss reviews --json
   pgrep -fl 'polygloss(-cli)? wait'
   sqlite3 "\$POLYGLOSS_DATA_DIR/polygloss.db" 'select session_id, pid from waiters'

4. Cleanup after the last case: quit Claude Code and Polygloss, then in each
   scratch repo you installed the plugin in

   claude plugin uninstall polygloss@polygloss --scope local

   and once

   claude plugin marketplace remove polygloss
   rm -rf $(q "$root")
EOF
