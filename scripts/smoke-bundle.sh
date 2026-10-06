#!/usr/bin/env bash
# Smoke test of a packaged Polygloss.app (plan T5.1).
#
#   scripts/smoke-bundle.sh [--static] <path/to/Polygloss.app>
#
# Static checks (all that --static runs): both executables are Mach-O in
# Contents/MacOS; Info.plist has the bundle id, main executable, URL scheme,
# minimum macOS and an explicit CFBundleVersion equal to the crate version
# (never cargo-packager's timestamp); the licenses and third-party notices
# are in Contents/Resources (plan T5.7); the signature (ad-hoc or Developer ID)
# seals the bundle; Sparkle (T5.3) is embedded exactly when SUFeedURL and
# SUPublicEDKey are set, without its XPC services; library validation is
# relaxed (packaging/entitlements-adhoc.plist) only in an ad-hoc bundle that
# embeds Sparkle, never under a Developer ID.
#
# Then it runs the bundle from where it is (never /Applications) in a sandbox:
# a throwaway HOME, data dir, config and git config (plan: Global constraints,
# "Test hygiene"), POLYGLOSS_TEST=1 for the `debug_state` op. It records a
# commit review of a fixture repo with the bundled CLI, launches the app with
# `open -g`, sends `hello` over the app socket, opens `polygloss://diff/<id>`
# through LaunchServices and confirms the tab via `debug_state` (whose
# `updater` must be "idle" when Sparkle is embedded: loaded, never started in
# test mode, so nothing is checked or prompted; none otherwise), then quits
# the app and unregisters the bundle from LaunchServices again.
set -euo pipefail

usage() {
  echo "usage: scripts/smoke-bundle.sh [--static] <path/to/Polygloss.app>" >&2
  exit 2
}

static_only=0
app=""
for arg in "$@"; do
  case "$arg" in
    --static) static_only=1 ;;
    -*) usage ;;
    *)
      [ -z "$app" ] || usage
      app="$arg"
      ;;
  esac
done
[ -n "$app" ] || usage

fail() {
  echo "smoke-bundle: FAIL: $*" >&2
  exit 1
}
ok() { echo "smoke-bundle: ok: $*"; }

[ -d "$app" ] || fail "no such bundle: $app"
app="$(cd "$app" && pwd -P)"
contents="$app/Contents"
plist="$contents/Info.plist"
[ -f "$plist" ] || fail "missing Contents/Info.plist"

# ---- static checks ------------------------------------------------------

for exe in Polygloss polygloss-cli; do
  path="$contents/MacOS/$exe"
  [ -f "$path" ] && [ -x "$path" ] || fail "missing executable Contents/MacOS/$exe"
  case "$(file -b "$path")" in
    *Mach-O*) ;;
    *) fail "Contents/MacOS/$exe is not a Mach-O executable" ;;
  esac
done
ok "Contents/MacOS has Polygloss and polygloss-cli"

# One Info.plist value (raw), or empty when the key is missing. Only plutil's
# exit status tells: on macOS 15 it prints its "Could not extract value" error
# to stdout, which, taken for a value, sent the URL scheme loop below past the
# end of the array forever (CI's bun job hung for an hour).
plist_get() {
  local value
  value="$(plutil -extract "$1" raw -o - "$plist" 2>/dev/null)" && printf '%s' "$value"
  return 0
}

expect_key() { # <key> <expected>
  local got
  got="$(plist_get "$1")"
  [ "$got" = "$2" ] || fail "Info.plist $1 is '${got}', expected '$2'"
}
expect_key CFBundleIdentifier dev.dak.polygloss
expect_key CFBundleExecutable Polygloss
expect_key CFBundlePackageType APPL
expect_key LSMinimumSystemVersion 14.0

short_version="$(plist_get CFBundleShortVersionString)"
[[ "$short_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+].*)?$ ]] ||
  fail "Info.plist CFBundleShortVersionString '$short_version' is not the crate version"
# CFBundleVersion: the crate version without pre-release or build metadata
# (Sparkle compares it; LaunchServices wants dot-separated integers).
expect_key CFBundleVersion "${short_version%%[-+]*}"

schemes=""
i=0
while count="$(plist_get "CFBundleURLTypes.$i")" && [ -n "$count" ]; do
  j=0
  while scheme="$(plist_get "CFBundleURLTypes.$i.CFBundleURLSchemes.$j")" && [ -n "$scheme" ]; do
    schemes="$schemes $scheme"
    j=$((j + 1))
  done
  i=$((i + 1))
done
[[ " $schemes " == *" polygloss "* ]] ||
  fail "Info.plist CFBundleURLTypes does not register the polygloss scheme"
ok "Info.plist: dev.dak.polygloss $short_version, polygloss:// scheme, macOS 14.0+"

for f in LICENSE-MIT LICENSE-APACHE NOTICE third-party-notices.md; do
  [ -s "$contents/Resources/$f" ] || fail "missing Contents/Resources/$f (licenses, plan T5.7)"
done
ok "Contents/Resources has the licenses and third-party notices"

codesign --verify --deep --strict "$app" 2>/dev/null ||
  fail "the code signature does not seal the bundle (codesign --verify --deep --strict)"
ok "signature seals the bundle"

sparkle_fw="$contents/Frameworks/Sparkle.framework"
feed_url="$(plist_get SUFeedURL)"
public_ed_key="$(plist_get SUPublicEDKey)"
sparkle=0
if [ -d "$sparkle_fw" ]; then
  [ -n "$feed_url" ] && [ -n "$public_ed_key" ] ||
    fail "Sparkle.framework is embedded but Info.plist lacks SUFeedURL or SUPublicEDKey"
  [ ! -e "$sparkle_fw/Versions/B/XPCServices" ] ||
    fail "Sparkle.framework still has its XPC services (only for sandboxed apps)"
  sparkle=1
  ok "Sparkle embedded; updates from $feed_url"
elif [ -n "$feed_url$public_ed_key" ]; then
  fail "Info.plist has SUFeedURL or SUPublicEDKey but no Sparkle.framework is embedded"
else
  ok "no updater (built without an appcast)"
fi

team="$(codesign --display --verbose=2 "$app" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
entitlements="$(codesign --display --entitlements - --xml "$app" 2>/dev/null || true)"
if [[ "$entitlements" == *com.apple.security.cs.disable-library-validation* ]]; then
  [ "$team" = "not set" ] ||
    fail "the $team signature disables library validation (Developer ID builds use packaging/entitlements.plist)"
  [ "$sparkle" = 1 ] || fail "library validation is disabled without Sparkle embedded"
  ok "ad-hoc bundle: library validation relaxed for the ad-hoc Sparkle.framework"
fi

echo "smoke-bundle: static checks passed"
[ "$static_only" = 1 ] && exit 0

# ---- runtime checks -----------------------------------------------------

lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
app_exe="$contents/MacOS/Polygloss"
cli="$contents/MacOS/polygloss-cli"

if pgrep -f "^$app_exe" >/dev/null; then
  fail "this bundle is already running; quit it first (smoke-bundle launches its own)"
fi

# The sandbox: short (the socket path must fit in sun_path, 104 bytes).
root="$(mktemp -d /tmp/pgsmoke-XXXXXX)"
root="$(cd "$root" && pwd -P)"
app_pid=""
cleanup() {
  if [ -n "$app_pid" ] && kill -0 "$app_pid" 2>/dev/null; then
    kill -TERM "$app_pid" 2>/dev/null || true
    for _ in $(seq 50); do
      kill -0 "$app_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$app_pid" 2>/dev/null || true
  fi
  "$lsregister" -u "$app" >/dev/null 2>&1 || true
  rm -rf "$root"
}
trap cleanup EXIT

home="$root/home"
data="$root/data"
mkdir -m 700 -p "$home/.config" "$home/.cache" "$data"
: >"$root/gitconfig"
sandbox_env=(
  "HOME=$home"
  "POLYGLOSS_DATA_DIR=$data"
  "POLYGLOSS_CACHE_DIR=$root/cache"
  "POLYGLOSS_LOG_DIR=$root/logs"
  "XDG_CONFIG_HOME=$home/.config"
  "XDG_CACHE_HOME=$home/.cache"
  "GIT_CONFIG_GLOBAL=$root/gitconfig"
  "GIT_CONFIG_NOSYSTEM=1"
  "POLYGLOSS_TEST=1"
  "PATH=${PATH}:/usr/bin:/bin:/usr/sbin:/sbin"
  "TMPDIR=${TMPDIR:-/tmp}"
  "LANG=${LANG:-en_US.UTF-8}"
  "GIT_AUTHOR_NAME=Polygloss Smoke"
  "GIT_AUTHOR_EMAIL=smoke@polygloss.invalid"
  "GIT_AUTHOR_DATE=2026-01-01T00:00:00Z"
  "GIT_COMMITTER_NAME=Polygloss Smoke"
  "GIT_COMMITTER_EMAIL=smoke@polygloss.invalid"
  "GIT_COMMITTER_DATE=2026-01-01T00:00:00Z"
)
in_sandbox() { env -i "${sandbox_env[@]}" "$@"; }
# `open` passes its environment on, and --env makes it explicit.
open_env=()
for kv in "${sandbox_env[@]}"; do open_env+=(--env "$kv"); done

socket="$data/polygloss.sock"

# One JSON-lines request over the app socket (design §13.3); prints the
# response line. Exits non-zero when nothing answers within 5 s.
sock_call() { # <op> [<extra json members>]
  local req="{\"v\":1,\"id\":1,\"op\":\"$1\"${2:+,$2}}"
  /usr/bin/perl -MIO::Socket::UNIX -e '
    my ($path, $req) = @ARGV;
    my $s = IO::Socket::UNIX->new(Type => SOCK_STREAM(), Peer => $path) or exit 3;
    $s->autoflush(1);
    print $s "$req\n";
    local $SIG{ALRM} = sub { exit 4 };
    alarm 5;
    my $line = <$s>;
    defined $line or exit 5;
    print $line;' "$socket" "$req"
}

# Evaluates a Perl expression over the decoded JSON `$j`; prints the result.
json_eval() { # <json> <expr>
  /usr/bin/perl -MJSON::PP -e '
    my $j = JSON::PP::decode_json($ARGV[0]);
    my $v = eval $ARGV[1];
    die $@ if $@;
    print defined $v ? $v : "";' "$1" "$2"
}

# Both executables report the bundle's version.
app_version="$(in_sandbox "$app_exe" --version)"
[ "$app_version" = "Polygloss $short_version" ] ||
  fail "Polygloss --version printed '$app_version', expected 'Polygloss $short_version'"
cli_version="$(in_sandbox "$cli" --version)"
[ "$cli_version" = "polygloss $short_version" ] ||
  fail "polygloss-cli --version printed '$cli_version', expected 'polygloss $short_version'"
ok "both executables report $short_version"

# A fixture repo with two commits; the bundled CLI records the HEAD commit's
# review without launching the app.
repo="$home/repo"
mkdir "$repo"
in_sandbox git -C "$repo" init -q -b main
printf 'one\n' >"$repo/a.txt"
in_sandbox git -C "$repo" add a.txt
in_sandbox git -C "$repo" commit -q -m one
printf 'one\ntwo\n' >"$repo/a.txt"
in_sandbox git -C "$repo" commit -q -am two
shown="$(in_sandbox "$cli" --json --no-open --repo "$repo" show HEAD)" ||
  fail "polygloss-cli show HEAD failed: $shown"
diff_id="$(json_eval "$shown" '$j->{diff_id}')"
[[ "$diff_id" =~ ^[0-9a-f]{64}$ ]] || fail "polygloss-cli show printed no diff_id: $shown"
ok "bundled CLI recorded diff $diff_id"

# Launch through LaunchServices, in the background, without restoring windows.
in_sandbox /usr/bin/open -g -F -a "$app" "${open_env[@]}" ||
  fail "open -g -a $app failed"
hello=""
for _ in $(seq 600); do
  if [ -S "$socket" ] && hello="$(sock_call hello '"client":"smoke-bundle"' 2>/dev/null)"; then
    break
  fi
  hello=""
  sleep 0.1
done
[ -n "$hello" ] || fail "the app did not answer hello on $socket within 60 s"
app_pid="$(json_eval "$hello" '$j->{result}{pid}')"
hello_version="$(json_eval "$hello" '$j->{result}{version}')"
[ "$hello_version" = "$short_version" ] ||
  fail "hello reported version '$hello_version', expected '$short_version': $hello"
running_exe="$(ps -o comm= -p "$app_pid" || true)"
[ "$running_exe" = "$app_exe" ] ||
  fail "the app answering hello (pid $app_pid) runs '$running_exe', not $app_exe"
ok "app $app_pid answered hello over the socket"

link="$data/bin/polygloss"
[ -L "$link" ] && [ "$(realpath "$link")" = "$cli" ] ||
  fail "$link does not point at the bundle's polygloss-cli"
ok "the stable CLI link points at the bundle"

in_sandbox /usr/bin/open -g -a "$app" "polygloss://diff/$diff_id" ||
  fail "open polygloss://diff/$diff_id failed"
tab=""
for _ in $(seq 300); do
  state="$(sock_call debug_state 2>/dev/null || true)"
  if [ -n "$state" ]; then
    tab="$(json_eval "$state" "join ',', map { \$_->{review_id} } grep { \$_->{diff_id} eq '$diff_id' } @{\$j->{result}{tabs}}")"
    [ -n "$tab" ] && break
  fi
  sleep 0.1
done
[ -n "$tab" ] || fail "polygloss://diff/$diff_id opened no tab within 30 s; last debug_state: ${state:-none}"
ok "polygloss://diff/$diff_id opened review $tab"

updater="$(json_eval "$state" '$j->{result}{updater} // "none"')"
if [ "$sparkle" = 1 ]; then
  [ "$updater" = idle ] ||
    fail "Sparkle is embedded but the app reports updater '$updater', expected 'idle'; app log: $(grep -h -i -e sparkle -e updater "$root"/logs/* 2>/dev/null | tail -n 5)"
  ok "Sparkle.framework loaded (updater idle in test mode)"
else
  [ "$updater" = none ] || fail "no Sparkle embedded, yet the app reports updater '$updater'"
fi

kill -TERM "$app_pid"
for _ in $(seq 100); do
  kill -0 "$app_pid" 2>/dev/null || break
  sleep 0.1
done
kill -0 "$app_pid" 2>/dev/null && fail "the app (pid $app_pid) did not quit on SIGTERM"
app_pid=""
ok "app quit"

echo "smoke-bundle: PASS ($app)"
