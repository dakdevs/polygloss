#!/usr/bin/env bash
# Sets every GitHub secret and variable that .github/workflows/release.yml
# needs (ADR-0019), from 1Password through the `op` CLI:
#
#   scripts/setup-release-secrets.sh [--repo <owner/name>] [--dry-run]
#
# From the vault "dak.dev" of the account my.1password.com:
#
#   APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD, APPLE_SIGNING_IDENTITY
#       the attachment developer-id-application.p12 (base64) and the field
#       "p12 password" of "Polygloss · Apple Developer ID Application (Dak
#       Washbrook, 5U7E4UQ5M3)"; the identity is the certificate's name. A
#       certificate of any team but 5U7E4UQ5M3 (by name or OU) is refused,
#       FCSF68W94H by name, before anything is set;
#   APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_PRIVATE_KEY
#       the fields "Key ID" and "Issuer ID" and the attachment
#       AuthKey_<Key ID>.p8 of "Polygloss · App Store Connect API key
#       (notarization, 87T26G474H)", for notarytool;
#   SPARKLE_PRIVATE_ED_KEY, and the variable SPARKLE_PUBLIC_ED_KEY
#       the fields "private key" and "public key" of "Polygloss · Sparkle
#       EdDSA update signing key". When that item does not exist, Sparkle's
#       generate_keys (scripts/fetch-sparkle.sh fetches it into
#       vendor/sparkle-bin/, or $SPARKLE_BIN_DIR) makes the key pair in the
#       login keychain under the account dev.dak.polygloss (or exports the one
#       already there), and the pair is saved as that item first.
#
# The optional variables POLYGLOSS_APPCAST_URL (default: the latest release's
# appcast.xml) and HOMEBREW_TAP_REPO and secret HOMEBREW_TAP_TOKEN (no tap
# bump without them) are left alone. --repo defaults to dakdevs/polygloss.
#
# Values go to `gh secret set` / `gh variable set` and `op item create` on
# stdin, never on a command line, and are never printed. Files live in a
# private temp dir that is removed on every exit, including an interrupt.
# --dry-run lists what it would read and set, and reads nothing.
set -euo pipefail

team=5U7E4UQ5M3
refused_team=FCSF68W94H # another team's Developer ID: never used (ADR-0019)
account=my.1password.com
vault=dak.dev
cert_item="Polygloss · Apple Developer ID Application (Dak Washbrook, 5U7E4UQ5M3)"
api_item="Polygloss · App Store Connect API key (notarization, 87T26G474H)"
sparkle_item="Polygloss · Sparkle EdDSA update signing key"
sparkle_account=dev.dak.polygloss
secrets=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY
  APPLE_API_KEY APPLE_API_ISSUER APPLE_API_PRIVATE_KEY SPARKLE_PRIVATE_ED_KEY)
variables=(SPARKLE_PUBLIC_ED_KEY)

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$repo_root"

say() { echo "setup-release-secrets: $*"; }
die() {
  echo "setup-release-secrets: $*" >&2
  exit 1
}
usage() {
  echo "usage: scripts/setup-release-secrets.sh [--repo <owner/name>] [--dry-run]" >&2
  exit 2
}

repo=dakdevs/polygloss dry_run=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry_run=1 && shift ;;
    --repo)
      [ $# -ge 2 ] || usage
      repo="$2" && shift 2
      ;;
    *) usage ;;
  esac
done
[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || usage

for tool in op gh jq /usr/bin/openssl; do
  command -v "$tool" >/dev/null || die "needs $tool on PATH"
done

if [ "$dry_run" = 1 ]; then
  say "dry run: would read, from 1Password ($account, vault $vault):"
  for item in "$cert_item" "$api_item" "$sparkle_item"; do echo "  item     $item"; done
  say "and set, on $repo:"
  for s in "${secrets[@]}"; do echo "  secret   $s"; done
  for v in "${variables[@]}"; do echo "  variable $v"; done
  exit 0
fi

umask 077
tmpbase="${TMPDIR:-/tmp}"
tmp="$(mktemp -d "${tmpbase%/}/polygloss-release-secrets.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT # bash runs it on HUP, INT and TERM too

# --- 1Password ---------------------------------------------------------------
op_() { op "$@" --account "$account"; }
# Item titles with "·", "(" or "," cannot be in a secret reference: use ids.
items="$(op_ item list --vault "$vault" --format json)" ||
  die "op item list failed: sign in with \`op signin --account $account\`"
item_id() { # <title>; empty when there is no such item
  local ids
  ids="$(jq -r --arg t "$1" '.[] | select(.title == $t) | .id' <<<"$items")"
  [ "$(grep -c . <<<"$ids")" -le 1 ] || die "more than one 1Password item is titled \"$1\""
  echo "$ids"
}
read_field() { # <item id> <field or file> [out file]; the value on stdout or in the file
  if [ $# -eq 3 ]; then
    op_ read --no-newline --out-file "$3" "op://$vault/$1/$2" >/dev/null
  else
    op_ read --no-newline "op://$vault/$1/$2"
  fi
}

cert_id="$(item_id "$cert_item")"
[ -n "$cert_id" ] || die "no 1Password item \"$cert_item\" in $vault"
api_id="$(item_id "$api_item")"
[ -n "$api_id" ] || die "no 1Password item \"$api_item\" in $vault"

say "reading the Developer ID certificate"
p12="$tmp/developer-id-application.p12"
read_field "$cert_id" developer-id-application.p12 "$p12" ||
  die "could not read developer-id-application.p12 from \"$cert_item\""
p12_password="$(read_field "$cert_id" "p12 password")" ||
  die "could not read the p12 password from \"$cert_item\""
pem="$(printf '%s' "$p12_password" |
  /usr/bin/openssl pkcs12 -in "$p12" -nokeys -clcerts -passin stdin 2>/dev/null)" ||
  die "developer-id-application.p12 does not open with its p12 password"
[ "$(grep -c 'BEGIN CERTIFICATE' <<<"$pem")" = 1 ] ||
  die "developer-id-application.p12 must hold exactly one certificate"
subject="$(/usr/bin/openssl x509 -noout -subject -nameopt sep_multiline,sname,utf8 <<<"$pem")"
identity="$(sed -n 's/^ *CN=//p' <<<"$subject" | head -n 1)"
ous="$(sed -n 's/^ *OU=//p' <<<"$subject")"
name_team=""
if [[ "$identity" =~ \(([A-Z0-9]{10})\)$ ]]; then name_team="${BASH_REMATCH[1]}"; fi
case "$name_team $ous" in
  *"$refused_team"*) die "refusing team $refused_team ($identity): that Developer ID belongs to another team" ;;
esac
[ "$name_team" = "$team" ] && [ "$ous" = "$team" ] ||
  die "the certificate is \"$identity\" (OU ${ous//$'\n'/ }), not team $team: refusing it"
case "$identity" in
  "Developer ID Application: "*) ;;
  *) die "\"$identity\" is not a Developer ID Application certificate" ;;
esac

say "reading the App Store Connect API key"
key_id="$(read_field "$api_id" "Key ID")" || die "could not read Key ID from \"$api_item\""
issuer="$(read_field "$api_id" "Issuer ID")" || die "could not read Issuer ID from \"$api_item\""
[[ "$key_id" =~ ^[A-Z0-9]{10}$ ]] || die "the Key ID of \"$api_item\" is not an App Store Connect key id"
[[ "$issuer" =~ ^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$ ]] ||
  die "the Issuer ID of \"$api_item\" is not a UUID"
p8="$tmp/AuthKey.p8"
read_field "$api_id" "AuthKey_$key_id.p8" "$p8" ||
  die "could not read AuthKey_<Key ID>.p8 from \"$api_item\""
grep -q 'BEGIN PRIVATE KEY' "$p8" || die "the .p8 of \"$api_item\" is not a private key"

sparkle_private="$tmp/sparkle-private-key"
sparkle_id="$(item_id "$sparkle_item")"
if [ -n "$sparkle_id" ]; then
  say "reading the Sparkle key pair"
  read_field "$sparkle_id" "private key" "$sparkle_private" ||
    die "could not read the private key from \"$sparkle_item\""
  public_ed_key="$(read_field "$sparkle_id" "public key")" ||
    die "could not read the public key from \"$sparkle_item\""
else
  sparkle_bin="${SPARKLE_BIN_DIR:-$repo_root/vendor/sparkle-bin}"
  [ -x "$sparkle_bin/generate_keys" ] || scripts/fetch-sparkle.sh
  [ -x "$sparkle_bin/generate_keys" ] || die "no $sparkle_bin/generate_keys"
  say "making the Sparkle key pair (login keychain, account $sparkle_account)"
  "$sparkle_bin/generate_keys" --account "$sparkle_account" >/dev/null ||
    die "generate_keys failed"
  public_ed_key="$("$sparkle_bin/generate_keys" --account "$sparkle_account" -p | tail -n 1)" ||
    die "generate_keys -p failed"
  "$sparkle_bin/generate_keys" --account "$sparkle_account" -x "$sparkle_private" >/dev/null ||
    die "generate_keys -x failed"
fi
[ -s "$sparkle_private" ] || die "no Sparkle private key"
[ "$(printf '%s' "$public_ed_key" | /usr/bin/base64 -D 2>/dev/null | wc -c | tr -d ' ')" = 32 ] ||
  die "the Sparkle public key is not a base64 Ed25519 key"
if [ -z "$sparkle_id" ]; then
  say "saving the Sparkle key pair as \"$sparkle_item\""
  jq -n --arg title "$sparkle_item" --rawfile private "$sparkle_private" \
    --arg public "$public_ed_key" --arg repo "$repo" '{
      title: $title,
      category: "SECURE_NOTE",
      fields: [
        {id: "notesPlain", type: "STRING", purpose: "NOTES", label: "notesPlain",
         value: "Sparkle EdDSA (ed25519) key pair that signs Polygloss updates. The private key is the SPARKLE_PRIVATE_ED_KEY secret of \($repo); the public key is its SPARKLE_PUBLIC_ED_KEY variable and the SUPublicEDKey of every bundle. Made by scripts/setup-release-secrets.sh."},
        {id: "private_key", type: "CONCEALED", label: "private key", value: ($private | rtrimstr("\n"))},
        {id: "public_key", type: "STRING", label: "public key", value: $public}
      ]}' | op_ item create --vault "$vault" - >/dev/null ||
    die "could not save \"$sparkle_item\" in 1Password; nothing was set on GitHub"
fi

# --- GitHub --------------------------------------------------------------------
put() { # <secret|variable> <name>; the value on stdin
  gh "$1" set "$2" --repo "$repo" >/dev/null || die "gh $1 set $2 failed"
  say "set $1 $2"
}
say "setting the release secrets on $repo"
/usr/bin/base64 -i "$p12" | put secret APPLE_CERTIFICATE
printf '%s' "$p12_password" | put secret APPLE_CERTIFICATE_PASSWORD
printf '%s' "$identity" | put secret APPLE_SIGNING_IDENTITY
printf '%s' "$key_id" | put secret APPLE_API_KEY
printf '%s' "$issuer" | put secret APPLE_API_ISSUER
put secret APPLE_API_PRIVATE_KEY <"$p8"
tr -d '\n' <"$sparkle_private" | put secret SPARKLE_PRIVATE_ED_KEY
printf '%s' "$public_ed_key" | put variable SPARKLE_PUBLIC_ED_KEY
say "done: the next push to main that passes CI is released"
