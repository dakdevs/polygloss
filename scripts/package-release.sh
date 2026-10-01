#!/usr/bin/env bash
# Builds the release bundle and DMG (plan T5.1, design §21):
#
#   scripts/package-release.sh [--sign]
#
#   1. `cargo build --release`, one `-p` each for polygloss-app and
#      polygloss-cli (a joint build unifies features and would link
#      polygloss-platform's `appkit` into the CLI);
#   2. `cargo packager --release --formats app` lays out Polygloss.app from them
#      (config: crates/polygloss-app/Cargo.toml, [package.metadata.packager]);
#   3. stamps CFBundleVersion with the crate version (cargo-packager writes a
#      timestamp; Sparkle compares CFBundleVersion); with an appcast configured
#      (POLYGLOSS_APPCAST_URL and SPARKLE_PUBLIC_ED_KEY, both or neither, plan
#      T5.3) it also writes SUFeedURL and SUPublicEDKey and copies
#      vendor/Sparkle.framework (scripts/fetch-sparkle.sh) into
#      Contents/Frameworks; without one the bundle has no updater;
#   4. signs ad-hoc with the hardened runtime, inside out (Sparkle's nested
#      code via scripts/sign-sparkle.sh, polygloss-cli, then the bundle), so
#      the bundle is sealed even without credentials. Library validation
#      rejects an ad-hoc framework in an ad-hoc app (neither has a team ID),
#      so an ad-hoc bundle that embeds Sparkle is signed with
#      packaging/entitlements-adhoc.plist (disable-library-validation); a
#      Developer ID signature replaces it with packaging/entitlements.plist
#      (no exceptions), and smoke-bundle.sh --static enforces both; with
#      --sign, scripts/sign-and-notarize.sh (T5.2) then re-signs it with the
#      Developer ID from the environment, notarizes and staples it (without
#      credentials it prints "signing skipped: no credentials" and keeps the
#      ad-hoc signature);
#   5. makes Polygloss_<version>_aarch64.dmg (the app plus an /Applications
#      link) with hdiutil, which --sign also hands to sign-and-notarize.sh;
#   6. runs scripts/smoke-bundle.sh --static on the result.
#
# Step 3 also copies LICENSE-MIT, LICENSE-APACHE, NOTICE and
# packaging/third-party-notices.md (plan T5.7) into Contents/Resources.
#
# Outputs go to dist/ (gitignored), or $POLYGLOSS_DIST_DIR. Needs
# cargo-packager 0.11.8: `cargo install cargo-packager --version =0.11.8 --locked`.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

sign=0
for arg in "$@"; do
  case "$arg" in
    --sign) sign=1 ;;
    *)
      echo "usage: scripts/package-release.sh [--sign]" >&2
      exit 2
      ;;
  esac
done

say() { echo "package-release: $*"; }
die() {
  echo "package-release: $*" >&2
  exit 1
}

# The whole run holds cargo.sh's build-dir lock: otherwise another checkout
# claiming the shared build dir between the two builds (or before packaging)
# cleans the first build's binaries out of this checkout's target dir.
if [ "${POLYGLOSS_PACKAGE_RELEASE_LOCKED-}" != 1 ]; then
  POLYGLOSS_PACKAGE_RELEASE_LOCKED=1 exec scripts/cargo.sh with-lock "$repo_root/scripts/package-release.sh" "$@"
fi

# Sparkle (T5.3): both appcast values or neither; checked before building.
appcast_url="${POLYGLOSS_APPCAST_URL-}"
public_ed_key="${SPARKLE_PUBLIC_ED_KEY-}"
sparkle_src="${POLYGLOSS_SPARKLE_FRAMEWORK:-$repo_root/vendor/Sparkle.framework}"
sparkle=0
if [ -n "$appcast_url" ] || [ -n "$public_ed_key" ]; then
  [ -n "$appcast_url" ] && [ -n "$public_ed_key" ] ||
    die "set both POLYGLOSS_APPCAST_URL and SPARKLE_PUBLIC_ED_KEY for updates, or neither"
  case "$appcast_url" in
    https://*) ;;
    *) die "POLYGLOSS_APPCAST_URL must be an https URL, not '$appcast_url'" ;;
  esac
  key_bytes="$(printf '%s' "$public_ed_key" | /usr/bin/base64 -D 2>/dev/null | wc -c | tr -d ' ')"
  [ "$key_bytes" = 32 ] ||
    die "SPARKLE_PUBLIC_ED_KEY is not a base64 Ed25519 public key (32 bytes; Sparkle's generate_keys prints it)"
  [ -f "$sparkle_src/Versions/B/Autoupdate" ] ||
    die "no Sparkle.framework at $sparkle_src: run scripts/fetch-sparkle.sh"
  sparkle=1
fi

signer="$repo_root/scripts/sign-and-notarize.sh"

dist="${POLYGLOSS_DIST_DIR:-$repo_root/dist}"
mkdir -p "$dist"
dist="$(cd "$dist" && pwd -P)"
app="$dist/Polygloss.app"
entitlements="$repo_root/packaging/entitlements.plist"
app_entitlements="$entitlements"

say "building polygloss-app and polygloss-cli (release)"
scripts/cargo.sh build --release -p polygloss-app
scripts/cargo.sh build --release -p polygloss-cli

say "packaging $app"
rm -rf "$app"
# cargo-packager reads APPLE_* credentials (it imports the certificate into its
# own keychain, rewriting the user's search list, then signs and notarizes)
# whenever a signing identity reaches its config. None is configured, and it
# never sees the credentials either: signing is scripts/sign-and-notarize.sh's.
packager_env=()
for var in $(compgen -e); do
  case "$var" in APPLE_*) packager_env+=(-u "$var") ;; esac
done
env ${packager_env[@]+"${packager_env[@]}"} \
  scripts/cargo.sh packager --release --formats app --out-dir "$dist" ||
  die "cargo packager failed (needs: cargo install cargo-packager --version =0.11.8 --locked)"
[ -d "$app" ] || die "cargo packager made no $app"

plist="$app/Contents/Info.plist"
version="$(plutil -extract CFBundleShortVersionString raw -o - "$plist")"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+].*)?$ ]] ||
  die "CFBundleShortVersionString '$version' is not a crate version"
# Dot-separated integers only (LaunchServices, Sparkle): drop any pre-release.
plutil -replace CFBundleVersion -string "${version%%[-+]*}" "$plist"

# Licenses (plan T5.7): ours, the NOTICE and the generated third-party
# notices (scripts/third-party-notices.ts; CI checks it is current).
say "bundling the licenses and third-party notices"
mkdir -p "$app/Contents/Resources"
for f in LICENSE-MIT LICENSE-APACHE NOTICE packaging/third-party-notices.md; do
  [ -f "$repo_root/$f" ] || die "missing $f"
  cp "$repo_root/$f" "$app/Contents/Resources/"
done

sparkle_fw="$app/Contents/Frameworks/Sparkle.framework"
if [ "$sparkle" = 1 ]; then
  say "embedding Sparkle (updates from $appcast_url)"
  plutil -replace SUFeedURL -string "$appcast_url" "$plist"
  plutil -replace SUPublicEDKey -string "$public_ed_key" "$plist"
  mkdir -p "$app/Contents/Frameworks"
  ditto "$sparkle_src" "$sparkle_fw"
  app_entitlements="$repo_root/packaging/entitlements-adhoc.plist"
else
  say "no updater: POLYGLOSS_APPCAST_URL and SPARKLE_PUBLIC_ED_KEY are not set"
fi

say "signing ad-hoc (hardened runtime)"
adhoc() { # <path> <entitlements> [<identifier>]
  codesign --force --sign - --options runtime --entitlements "$2" \
    ${3:+--identifier "$3"} "$1"
}
if [ "$sparkle" = 1 ]; then
  scripts/sign-sparkle.sh "$sparkle_fw" -
fi
adhoc "$app/Contents/MacOS/polygloss-cli" "$entitlements" dev.dak.polygloss.cli
adhoc "$app" "$app_entitlements"
codesign --verify --deep --strict "$app"
if [ "$sign" = 1 ]; then
  say "signing and notarizing $app"
  "$signer" "$app"
fi

dmg="$dist/Polygloss_${version}_aarch64.dmg"
say "making $dmg"
staging="$dist/.dmg-staging"
rm -rf "$staging" "$dmg"
mkdir "$staging"
ditto "$app" "$staging/Polygloss.app"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname Polygloss -srcfolder "$staging" -fs HFS+ \
  -format UDZO -ov "$dmg"
rm -rf "$staging"
if [ "$sign" = 1 ]; then
  say "signing and notarizing $dmg"
  "$signer" "$dmg"
fi

scripts/smoke-bundle.sh --static "$app"
say "done: $app, $dmg"
