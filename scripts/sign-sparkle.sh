#!/usr/bin/env bash
# Code-signs Sparkle.framework inside out (plan T5.2, library-choices §14
# step 5, Sparkle's "Code Signing" docs):
#
#   scripts/sign-sparkle.sh <Sparkle.framework> <identity> [codesign args…]
#
#   1. Versions/B/XPCServices/Installer.xpc and Downloader.xpc, when present
#      (scripts/fetch-sparkle.sh deletes them; the Downloader keeps its own
#      entitlements);
#   2. Versions/B/Autoupdate;
#   3. Versions/B/Updater.app;
#   4. the framework itself.
#
# Every signature uses the hardened runtime. <identity> `-` signs ad-hoc
# (no timestamp); any other identity gets a secure timestamp. Extra arguments
# (e.g. `--keychain <path>`) go to every codesign call. Called by
# scripts/sign-and-notarize.sh with the Developer ID, and for ad-hoc builds by
# scripts/package-release.sh (T5.3).
set -euo pipefail

usage() {
  echo "usage: scripts/sign-sparkle.sh <Sparkle.framework> <identity> [codesign args…]" >&2
  exit 2
}
[ $# -ge 2 ] || usage
fw="${1%/}"
identity="$2"
shift 2
[ -n "$identity" ] || usage

die() {
  echo "sign-sparkle: $*" >&2
  exit 1
}

b="$fw/Versions/B"
[ -d "$b" ] || die "$fw is not a Sparkle.framework (no Versions/B)"

if [ "$identity" = - ]; then
  timestamp=--timestamp=none
else
  timestamp=--timestamp
fi

sign() { # <path> [codesign args…]
  local path="$1"
  shift
  codesign --force --sign "$identity" --options runtime "$timestamp" \
    ${extra[@]+"${extra[@]}"} "$@" "$path"
}
extra=("$@")

if [ -d "$b/XPCServices/Installer.xpc" ]; then
  sign "$b/XPCServices/Installer.xpc"
fi
if [ -d "$b/XPCServices/Downloader.xpc" ]; then
  sign "$b/XPCServices/Downloader.xpc" --preserve-metadata=entitlements
fi
[ -f "$b/Autoupdate" ] || die "$b/Autoupdate is missing"
sign "$b/Autoupdate"
[ -d "$b/Updater.app" ] || die "$b/Updater.app is missing"
sign "$b/Updater.app"
sign "$fw"
