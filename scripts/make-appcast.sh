#!/usr/bin/env bash
# Makes the Sparkle appcast for a release (plan T5.3, library-choices §14):
#
#   scripts/make-appcast.sh <dist dir>
#
# Runs Sparkle's generate_appcast (from scripts/fetch-sparkle.sh, in
# vendor/sparkle-bin/ or $SPARKLE_BIN_DIR) over the release DMG in <dist dir>
# (Polygloss_<version>_aarch64.dmg, alone in a staging folder), signing the
# update with the EdDSA private key in $SPARKLE_PRIVATE_ED_KEY, which goes to
# generate_appcast on stdin (`--ed-key-file -`), never on a command line.
# Enclosure URLs are $SPARKLE_DOWNLOAD_URL_PREFIX + the DMG's name (release.yml:
# https://github.com/<repo>/releases/download/<tag>/). The appcast's
# sparkle:version must equal the bundle's CFBundleVersion; the result is
# <dist dir>/appcast.xml, which release.yml uploads to the release, where
# POLYGLOSS_APPCAST_URL (…/releases/latest/download/appcast.xml) finds it.
# With SPARKLE_RELEASE_NOTES (a Markdown file, scripts/release-notes.ts
# --sparkle) the notes go next to the DMG under its name and are embedded in
# the appcast's <description>, which Sparkle's update dialog shows.
#
# Skips (exit 0) without SPARKLE_PRIVATE_ED_KEY, or when <dist
# dir>/Polygloss.app was built without an appcast (no SUFeedURL).
set -euo pipefail

usage() {
  echo "usage: scripts/make-appcast.sh <dist dir>" >&2
  exit 2
}
[ $# -eq 1 ] || usage
[ -d "$1" ] || usage
dist="$(cd "$1" && pwd -P)"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
say() { echo "make-appcast: $*"; }
die() {
  echo "make-appcast: $*" >&2
  exit 1
}

if [ -z "${SPARKLE_PRIVATE_ED_KEY-}" ]; then
  say "appcast skipped: no SPARKLE_PRIVATE_ED_KEY"
  exit 0
fi

plist="$dist/Polygloss.app/Contents/Info.plist"
bundle_version=""
if [ -f "$plist" ]; then
  if ! plutil -extract SUFeedURL raw -o - "$plist" >/dev/null 2>&1; then
    say "appcast skipped: Polygloss.app has no SUFeedURL (built without POLYGLOSS_APPCAST_URL)"
    exit 0
  fi
  bundle_version="$(plutil -extract CFBundleVersion raw -o - "$plist")"
fi

prefix="${SPARKLE_DOWNLOAD_URL_PREFIX-}"
[ -n "$prefix" ] || die "set SPARKLE_DOWNLOAD_URL_PREFIX (where the DMG is downloaded from)"
case "$prefix" in
  https://*) ;;
  *) die "SPARKLE_DOWNLOAD_URL_PREFIX must be an https URL, not '$prefix'" ;;
esac
case "$prefix" in */) ;; *) prefix="$prefix/" ;; esac

bin="${SPARKLE_BIN_DIR:-$repo_root/vendor/sparkle-bin}"
[ -x "$bin/generate_appcast" ] ||
  die "no $bin/generate_appcast: run scripts/fetch-sparkle.sh first"

shopt -s nullglob
dmgs=("$dist"/Polygloss_*_aarch64.dmg)
shopt -u nullglob
[ "${#dmgs[@]}" -eq 1 ] || die "expected one Polygloss_*_aarch64.dmg in $dist, found ${#dmgs[@]}: no Polygloss_*_aarch64.dmg to publish"

staging="$(mktemp -d "${TMPDIR:-/tmp}/make-appcast.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/archives"
cp "${dmgs[0]}" "$staging/archives/"
notes=()
if [ -n "${SPARKLE_RELEASE_NOTES-}" ]; then
  [ -s "$SPARKLE_RELEASE_NOTES" ] || die "no release notes at SPARKLE_RELEASE_NOTES=$SPARKLE_RELEASE_NOTES"
  cp "$SPARKLE_RELEASE_NOTES" "$staging/archives/$(basename "${dmgs[0]}" .dmg).md"
  notes=(--embed-release-notes)
fi

say "generating the appcast for $(basename "${dmgs[0]}")"
printf '%s' "$SPARKLE_PRIVATE_ED_KEY" |
  "$bin/generate_appcast" --ed-key-file - --download-url-prefix "$prefix" \
    ${notes[@]+"${notes[@]}"} -o "$staging/appcast.xml" "$staging/archives" ||
  die "generate_appcast failed"
[ -s "$staging/appcast.xml" ] || die "generate_appcast wrote no appcast"

if [ -n "$bundle_version" ] &&
  ! grep -q "<sparkle:version>$bundle_version</sparkle:version>" "$staging/appcast.xml"; then
  die "the appcast's sparkle:version is not the bundle's CFBundleVersion $bundle_version"
fi
if [ ${#notes[@]} -gt 0 ] && ! grep -q '<description' "$staging/appcast.xml"; then
  die "generate_appcast did not embed the release notes"
fi
cp "$staging/appcast.xml" "$dist/appcast.xml"
say "wrote $dist/appcast.xml"
