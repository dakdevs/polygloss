#!/usr/bin/env bash
# Renders the app icon (design §21) into the committed files the bundle uses:
#
#   packaging/icon.icns        assets/icons/polygloss.svg at every size macOS
#                              uses, 16 to 1024 px (CFBundleIconFile, the
#                              fallback);
#   packaging/assets.car       packaging/polygloss.icon (Icon Composer; light,
#                              dark and tinted) compiled by actool from Xcode
#                              26 or later (CFBundleIconName);
#   packaging/assets.car.json  its manifest: the SHA-256 of the .icon document
#                              and of assets.car, and the actool that made it.
#
#   scripts/make-icon.sh          renders all three
#   scripts/make-icon.sh --check  exits 0 if the manifest matches the .icon
#                                 document and assets.car, else says why
#
# Rerun it after editing the svg or the .icon document (keep the .icon's
# layers on the svg's paths) and commit all three. scripts/package-release.sh
# bundles assets.car only while --check passes, so releases never run actool
# (its helper daemon has crashed on a CI runner). The document's hash is
# the SHA-256 of `shasum -a 256` over its files but dotfiles, sorted by path.
# POLYGLOSS_ICON_DIR (default packaging/) holds polygloss.icon and the three
# outputs; tests point it elsewhere.
#
# Rendering needs rsvg-convert (librsvg; `brew install librsvg`) or
# ImageMagick's `magick`, the system's iconutil, and Xcode 26 or later.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
svg="$repo_root/assets/icons/polygloss.svg"
dir="${POLYGLOSS_ICON_DIR:-$repo_root/packaging}"
doc="$dir/polygloss.icon"
car="$dir/assets.car"
manifest="$dir/assets.car.json"

die() {
  echo "make-icon: $*" >&2
  exit 1
}
# A path for messages: repo-relative inside the repo.
show() { echo "${1#"$repo_root"/}"; }

check=0
case "$#:${1-}" in
  0:) ;;
  1:--check) check=1 ;;
  *)
    echo "usage: scripts/make-icon.sh [--check]" >&2
    exit 2
    ;;
esac
[ -d "$doc" ] || die "no Icon Composer document at $(show "$doc")"

doc_sha256() {
  (cd "$doc" && find . -type f ! -name '.*' -print0 | LC_ALL=C sort -z |
    xargs -0 shasum -a 256) | shasum -a 256 | cut -d ' ' -f 1
}
sha256() { shasum -a 256 <"$1" | cut -d ' ' -f 1; }

if [ "$check" = 1 ]; then
  stale() { die "$(show "$car") is stale: $*; run scripts/make-icon.sh"; }
  [ -f "$manifest" ] || stale "no $(show "$manifest")"
  [ -f "$car" ] || stale "no $(show "$car")"
  [ "$(doc_sha256)" = "$(plutil -extract source-sha256 raw -o - "$manifest")" ] ||
    stale "$(show "$doc") changed since it was compiled"
  [ "$(sha256 "$car")" = "$(plutil -extract assets-car-sha256 raw -o - "$manifest")" ] ||
    stale "it is not the file $(show "$manifest") records"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

xcrun actool --version >"$work/actool.plist" 2>"$work/actool.log" ||
  die "needs Xcode 26 or later: $(cat "$work/actool.log")"
actool_info() {
  /usr/libexec/PlistBuddy -c "Print :com.apple.actool.version:$1" "$work/actool.plist"
}
actool_version="$(actool_info short-bundle-version)"
[ "${actool_version%%.*}" -ge 26 ] ||
  die "actool $actool_version cannot compile Icon Composer documents: needs Xcode 26 or later"

if command -v rsvg-convert >/dev/null; then
  render() { rsvg-convert --width "$1" --height "$1" --output "$2" "$svg"; }
elif command -v magick >/dev/null; then
  render() { magick -background none -density 384 "$svg" -resize "$1x$1" "$2"; }
else
  die "needs rsvg-convert (brew install librsvg) or ImageMagick's magick"
fi

iconset="$work/icon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  render "$size" "$iconset/icon_${size}x${size}.png"
  render "$((size * 2))" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil --convert icns --output "$dir/icon.icns" "$iconset"
echo "make-icon: wrote $(show "$dir/icon.icns")"

# The bundle's minimum macOS (packaging/Info.plist); package-release.sh sets
# CFBundleIconName to the --app-icon name.
mkdir "$work/car"
if ! xcrun actool "$doc" --compile "$work/car" --platform macosx \
  --minimum-deployment-target 14.0 --app-icon polygloss \
  --output-partial-info-plist "$work/car/partial.plist" >"$work/actool.log" 2>&1 ||
  [ ! -f "$work/car/Assets.car" ]; then
  sed 's/^/  actool: /' "$work/actool.log" >&2
  die "actool made no Assets.car from $(show "$doc")"
fi
cp "$work/car/Assets.car" "$car"
cat >"$manifest" <<EOF
{
  "source": "polygloss.icon",
  "source-sha256": "$(doc_sha256)",
  "assets-car-sha256": "$(sha256 "$car")",
  "actool": "$actool_version ($(actool_info bundle-version))"
}
EOF
echo "make-icon: wrote $(show "$car") and $(show "$manifest")"
