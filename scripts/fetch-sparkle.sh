#!/usr/bin/env bash
# Fetches the official Sparkle release into vendor/ (plan T5.3,
# library-choices §14):
#
#   scripts/fetch-sparkle.sh [--force]
#
#   1. asks the GitHub release API for Sparkle's $SPARKLE_VERSION release and
#      reads the SHA-256 GitHub publishes for Sparkle-<version>.tar.xz; it
#      must equal the digest pinned here ($SPARKLE_SHA256);
#   2. downloads the asset and checks its SHA-256 against that digest
#      (a mismatch fails and leaves vendor/ untouched);
#   3. installs vendor/Sparkle.framework without its XPC services
#      (Versions/B/XPCServices and the top-level link: they are only for
#      sandboxed apps) and the release's command-line tools in
#      vendor/sparkle-bin/ (generate_appcast, sign_update, generate_keys, …),
#      and records `<version> sha256:<digest>` in vendor/sparkle.version.
#
# A matching vendor/sparkle.version means it was fetched: nothing is
# downloaded again unless --force. GITHUB_TOKEN, when set (CI), authenticates
# the API call through a header file, never on the command line.
# scripts/package-release.sh embeds the framework when the appcast is
# configured; scripts/make-appcast.sh runs the tools. This is a build step:
# the app itself never downloads anything but Sparkle's own update checks.
#
# Env: POLYGLOSS_VENDOR_DIR (default <repo>/vendor), SPARKLE_SHA256 (the pin;
# tests serve a stand-in release).
set -euo pipefail

SPARKLE_VERSION=2.10.0
# The digest GitHub publishes for Sparkle-2.10.0.tar.xz (the asset's
# `digest` in the release API, checked 2026-09-30).
PINNED_SHA256=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
asset="Sparkle-$SPARKLE_VERSION.tar.xz"
api_url="https://api.github.com/repos/sparkle-project/Sparkle/releases/tags/$SPARKLE_VERSION"
download_prefix="https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/"

force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    *)
      echo "usage: scripts/fetch-sparkle.sh [--force]" >&2
      exit 2
      ;;
  esac
done

say() { echo "fetch-sparkle: $*"; }
die() {
  echo "fetch-sparkle: $*" >&2
  exit 1
}

pin="${SPARKLE_SHA256:-$PINNED_SHA256}"
vendor="${POLYGLOSS_VENDOR_DIR:-$repo_root/vendor}"
stamp="$vendor/sparkle.version"
want_stamp="$SPARKLE_VERSION sha256:$pin"

if [ "$force" = 0 ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want_stamp" ] &&
  [ -d "$vendor/Sparkle.framework" ] && [ -x "$vendor/sparkle-bin/generate_appcast" ]; then
  say "Sparkle $SPARKLE_VERSION already fetched in $vendor (--force fetches it again)"
  exit 0
fi

tmp="$(mktemp -d "${TMPDIR:-/tmp}/fetch-sparkle.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

curl_args=(-fsSL --proto =https --retry 3)
if [ -n "${GITHUB_TOKEN-}" ]; then
  (umask 077 && printf 'Authorization: Bearer %s\n' "$GITHUB_TOKEN" >"$tmp/auth-header")
  api_auth=(-H "@$tmp/auth-header")
else
  api_auth=()
fi

say "reading the Sparkle $SPARKLE_VERSION release from the GitHub API"
curl "${curl_args[@]}" -H "Accept: application/vnd.github+json" \
  ${api_auth[@]+"${api_auth[@]}"} -o "$tmp/release.json" "$api_url" ||
  die "cannot read $api_url"

# The asset's entry (plutil reads JSON; it is in every macOS).
json() { plutil -extract "$1" raw -o - "$tmp/release.json" 2>/dev/null; }
digest=""
url=""
i=0
while name="$(json "assets.$i.name")"; do
  if [ "$name" = "$asset" ]; then
    digest="$(json "assets.$i.digest" || true)"
    url="$(json "assets.$i.browser_download_url" || true)"
    break
  fi
  i=$((i + 1))
done
[ -n "$url" ] || die "the $SPARKLE_VERSION release has no $asset"
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] ||
  die "the release API publishes no SHA-256 digest for $asset (got '${digest}')"
[ "$url" = "$download_prefix$asset" ] ||
  die "unexpected download URL for $asset: $url"
[ "${digest#sha256:}" = "$pin" ] ||
  die "the published digest $digest of $asset is not the pinned sha256:$pin"

say "downloading $url"
curl "${curl_args[@]}" -o "$tmp/$asset" "$url" || die "cannot download $url"
got="$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)"
[ "$got" = "$pin" ] ||
  die "checksum mismatch for $asset: got sha256:$got, published sha256:$pin"

mkdir "$tmp/x"
tar -xJf "$tmp/$asset" -C "$tmp/x" || die "cannot unpack $asset"
fw="$tmp/x/Sparkle.framework"
[ -f "$fw/Versions/B/Sparkle" ] || die "$asset has no Sparkle.framework/Versions/B/Sparkle"
[ -x "$tmp/x/bin/generate_appcast" ] || die "$asset has no bin/generate_appcast"
# Sparkle docs, "Removing the XPC Services": only sandboxed apps use them.
rm -rf "$fw/Versions/B/XPCServices" "$fw/XPCServices"

mkdir -p "$vendor"
rm -rf "$vendor/Sparkle.framework" "$vendor/sparkle-bin" "$stamp"
mv "$fw" "$vendor/Sparkle.framework"
mv "$tmp/x/bin" "$vendor/sparkle-bin"
printf '%s\n' "$want_stamp" >"$stamp"
say "Sparkle $SPARKLE_VERSION in $vendor (Sparkle.framework, sparkle-bin/)"
