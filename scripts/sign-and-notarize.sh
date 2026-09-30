#!/usr/bin/env bash
# Signs, notarizes and staples Polygloss.app or its DMG with the Developer ID
# of team 5U7E4UQ5M3 (plan T5.2, design §21, ADR-0019):
#
#   scripts/sign-and-notarize.sh <Polygloss.app | Polygloss_<v>_aarch64.dmg>
#
# scripts/package-release.sh --sign calls it for the app (after its ad-hoc
# signature, before the DMG is made) and then for the DMG.
#
# Credentials come from the environment only:
#
#   signing       APPLE_CERTIFICATE (base64 .p12) + APPLE_CERTIFICATE_PASSWORD,
#                 imported into a temporary keychain; and/or
#                 APPLE_SIGNING_IDENTITY ("Developer ID Application: … (5U7E4UQ5M3)"),
#                 which alone signs from the user's own keychains
#   notarization  APPLE_API_KEY + APPLE_API_ISSUER + APPLE_API_KEY_PATH, or
#                 APPLE_KEYCHAIN_PROFILE (a `notarytool store-credentials` profile)
#
# Without signing credentials it prints `signing skipped: no credentials` and
# exits 0, leaving the ad-hoc signature. Without notarization credentials it
# signs but does not notarize, unless POLYGLOSS_REQUIRE_NOTARIZATION=1 (the
# release workflow), which makes that an error. It refuses any identity that is
# not a Developer ID Application of team 5U7E4UQ5M3 before touching a keychain.
#
# It never uses cargo-packager's signing: that imports certificates by itself
# and rewrites the user's keychain search list. Here the temporary keychain
# joins the search list only while signing, and the original list is restored
# on every exit.
set -euo pipefail

team=5U7E4UQ5M3
refused_team=FCSF68W94H # another team's Developer ID: never sign with it (ADR-0019)

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
entitlements="$(cd "$here/../packaging" && pwd -P)/entitlements.plist"

say() { echo "sign-and-notarize: $*"; }
die() {
  echo "sign-and-notarize: $*" >&2
  exit 1
}
usage() {
  echo "usage: scripts/sign-and-notarize.sh <Polygloss.app | .dmg>" >&2
  exit 2
}

[ $# -eq 1 ] || usage
target="${1%/}"
case "$target" in
  *.app) kind=app ;;
  *.dmg) kind=dmg ;;
  *) usage ;;
esac
[ -e "$target" ] || die "$target does not exist"
target="$(cd "$(dirname "$target")" && pwd -P)/$(basename "$target")"

cert="${APPLE_CERTIFICATE-}"
name="${APPLE_SIGNING_IDENTITY-}"
if [ -z "$cert" ] && [ -z "$name" ]; then
  say "signing skipped: no credentials ($target)"
  exit 0
fi

# --- Notarization credentials (checked before anything is signed) ----------
notary=()
if [ -n "${APPLE_API_KEY-}${APPLE_API_ISSUER-}${APPLE_API_KEY_PATH-}" ]; then
  if [ -z "${APPLE_API_KEY-}" ] || [ -z "${APPLE_API_ISSUER-}" ] || [ -z "${APPLE_API_KEY_PATH-}" ]; then
    die "notarizing with an API key needs all of APPLE_API_KEY, APPLE_API_ISSUER and APPLE_API_KEY_PATH"
  fi
  [ -f "$APPLE_API_KEY_PATH" ] || die "APPLE_API_KEY_PATH $APPLE_API_KEY_PATH does not exist"
  notary=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [ -n "${APPLE_KEYCHAIN_PROFILE-}" ]; then
  notary=(--keychain-profile "$APPLE_KEYCHAIN_PROFILE")
elif [ "${POLYGLOSS_REQUIRE_NOTARIZATION-}" = 1 ]; then
  die "notarization is required but no notarytool credentials are set" \
    "(APPLE_API_KEY + APPLE_API_ISSUER + APPLE_API_KEY_PATH, or APPLE_KEYCHAIN_PROFILE)"
fi

# --- The identity: team 5U7E4UQ5M3 only ------------------------------------
check_team() { # <team> <what>
  case "$1" in
    "$refused_team")
      die "refusing team $refused_team ($2): that Developer ID belongs to another team; polygloss signs only as team $team"
      ;;
    "$team") ;;
    "") die "$2 names no team; polygloss signs only as team $team" ;;
    *) die "$2 is team $1, not $team: refusing to sign" ;;
  esac
}
team_of_name() { # "Developer ID Application: Name (TEAMID)" -> TEAMID
  if [[ "$1" =~ \(([A-Z0-9]{10})\)$ ]]; then echo "${BASH_REMATCH[1]}"; fi
}

tmpbase="${TMPDIR:-/tmp}"
tmp="$(mktemp -d "${tmpbase%/}/polygloss-sign.XXXXXX")"
keychain=""
original_search_list=()
cleanup() {
  if [ -n "$keychain" ]; then
    security list-keychains -d user -s ${original_search_list[@]+"${original_search_list[@]}"} ||
      echo "sign-and-notarize: could not restore the keychain search list" >&2
    security delete-keychain "$keychain" ||
      echo "sign-and-notarize: could not delete $keychain" >&2
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

if [ -n "$cert" ]; then
  p12="$tmp/identity.p12"
  printf '%s' "$cert" | /usr/bin/base64 -D >"$p12" 2>/dev/null ||
    die "cannot read APPLE_CERTIFICATE: not base64"
  export POLYGLOSS_P12_PASSWORD="${APPLE_CERTIFICATE_PASSWORD-}"
  pem="$(/usr/bin/openssl pkcs12 -in "$p12" -nokeys -clcerts \
    -passin env:POLYGLOSS_P12_PASSWORD 2>/dev/null)" ||
    die "cannot read APPLE_CERTIFICATE: not a .p12, or the wrong APPLE_CERTIFICATE_PASSWORD"
  count="$(grep -c 'BEGIN CERTIFICATE' <<<"$pem" || true)"
  [ "$count" = 1 ] || die "cannot read APPLE_CERTIFICATE: it must hold exactly one identity (found $count)"
  subject="$(/usr/bin/openssl x509 -noout -subject -nameopt sep_multiline,sname,utf8 <<<"$pem")"
  cn="$(sed -n 's/^ *CN=//p' <<<"$subject" | head -n 1)"
  sha1="$(/usr/bin/openssl x509 -noout -fingerprint -sha1 <<<"$pem" | sed 's/.*=//; s/://g')"
  # The OU is the team Apple issued the certificate to; the name must agree.
  ous="$(sed -n 's/^ *OU=//p' <<<"$subject")"
  [ -n "$ous" ] || check_team "" "APPLE_CERTIFICATE ($cn)"
  while IFS= read -r ou; do
    check_team "$ou" "APPLE_CERTIFICATE ($cn)"
  done <<<"$ous"
  check_team "$(team_of_name "$cn")" "APPLE_CERTIFICATE ($cn)"
  if [ -n "$name" ] && [ "$name" != "$cn" ] && [ "$name" != "$sha1" ]; then
    die "APPLE_SIGNING_IDENTITY '$name' does not match APPLE_CERTIFICATE ($cn)"
  fi
  label="$cn"
  identity="$sha1" # unambiguous, whatever else the keychains hold
else
  check_team "$(team_of_name "$name")" "APPLE_SIGNING_IDENTITY '$name'"
  label="$name"
  identity="$name"
fi
case "$label" in
  "Developer ID Application: "*) ;;
  *) die "'$label' is not a Developer ID Application identity; only those can be notarized for distribution" ;;
esac

# --- Temporary keychain -----------------------------------------------------
keychain_args=()
if [ -n "$cert" ]; then
  search_list="$(security list-keychains -d user)"
  while IFS= read -r line; do
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line#\"}"
    line="${line%\"}"
    [ -n "$line" ] && original_search_list+=("$line")
  done <<<"$search_list"
  keychain="$tmp/signing.keychain-db"
  keychain_password="$(/usr/bin/openssl rand -hex 24)"
  say "importing $label into a temporary keychain"
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 21600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  security import "$p12" -k "$keychain" -P "${APPLE_CERTIFICATE_PASSWORD-}" \
    -f pkcs12 -x -T /usr/bin/codesign
  rm -f "$p12"
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
    -k "$keychain_password" "$keychain" >/dev/null
  # codesign finds the private key only through the search list; restored by
  # cleanup().
  security list-keychains -d user -s "$keychain" \
    ${original_search_list[@]+"${original_search_list[@]}"}
  keychain_args=(--keychain "$keychain")
fi

# --- Sign --------------------------------------------------------------------
sign() { # [codesign args…] <path>
  codesign --force --sign "$identity" --options runtime --timestamp \
    ${keychain_args[@]+"${keychain_args[@]}"} "$@"
}

if [ "$kind" = app ]; then
  say "signing $target as $label"
  sparkle="$target/Contents/Frameworks/Sparkle.framework"
  if [ -d "$sparkle" ]; then
    "$here/sign-sparkle.sh" "$sparkle" "$identity" \
      ${keychain_args[@]+"${keychain_args[@]}"}
  fi
  macos="$target/Contents/MacOS"
  sign --entitlements "$entitlements" --identifier dev.dak.polygloss.cli \
    "$macos/polygloss-cli"
  sign --entitlements "$entitlements" --identifier dev.dak.polygloss \
    "$macos/Polygloss"
  sign --entitlements "$entitlements" "$target"
else
  say "signing $target as $label"
  codesign --force --sign "$identity" --timestamp \
    ${keychain_args[@]+"${keychain_args[@]}"} "$target"
fi

codesign --verify --deep --strict --verbose=2 "$target"
details="$(codesign --display --verbose=2 "$target" 2>&1)"
grep -qx "TeamIdentifier=$team" <<<"$details" ||
  die "the signature of $target does not carry TeamIdentifier=$team"

# --- Notarize, staple, assess -----------------------------------------------
if [ ${#notary[@]} -eq 0 ]; then
  say "notarization skipped: no notarytool credentials ($target)"
  say "done: $target (signed, not notarized)"
  exit 0
fi

submission="$target"
if [ "$kind" = app ]; then
  submission="$tmp/$(basename "$target" .app).zip"
  ditto -c -k --sequesterRsrc --keepParent "$target" "$submission"
fi
say "notarizing $target (notarytool submit --wait)"
result="$tmp/notary.json"
if ! xcrun notarytool submit "$submission" --wait --output-format json \
  "${notary[@]}" >"$result"; then
  cat "$result" >&2
  die "notarytool submit failed for $target"
fi
status="$(plutil -extract status raw -o - "$result" 2>/dev/null || echo unknown)"
if [ "$status" != Accepted ]; then
  id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
  cat "$result" >&2
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" "${notary[@]}" >&2 || true
  fi
  die "notarization of $target was not accepted: $status"
fi

xcrun stapler staple "$target"
xcrun stapler validate "$target"
if [ "$kind" = app ]; then
  spctl --assess --type execute -vv "$target"
else
  spctl --assess --type open --context context:primary-signature -vv "$target"
fi
say "done: $target"
