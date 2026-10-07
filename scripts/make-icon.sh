#!/usr/bin/env bash
# Renders the app icon (design §21) into the committed files the bundle uses:
#
#   packaging/icon.icns        assets/icons/polygloss.svg at every size macOS
#                              uses, 16 to 1024 px (CFBundleIconFile, the
#                              fallback);
#   packaging/assets.car       packaging/polygloss.icon (Icon Composer; light,
#                              dark and tinted) compiled by actool from Xcode
#                              26 or later (CFBundleIconName);
#   packaging/assets.car.json  its manifest: what assets.car was compiled from
#                              and with, and its own SHA-256.
#
#   scripts/make-icon.sh          renders icon.icns, and compiles assets.car
#                                 only when --check fails
#   scripts/make-icon.sh --force  also recompiles a current assets.car
#   scripts/make-icon.sh --check  exits 0 if assets.car is current, else says
#                                 why
#
# Rerun it after editing the svg or the .icon document (keep the .icon's
# layers on the svg's paths) and commit what changed. actool's output is not
# byte-stable (two compiles of one document differ), so a current assets.car
# is kept, never recompiled for nothing. scripts/package-release.sh bundles
# assets.car only while --check passes, so releases never run actool (its
# helper daemon has crashed on a CI runner).
#
# assets.car is current while the manifest's
#   source-sha256      is the document's hash (below),
#   minimum-macos      is packaging/Info.plist's LSMinimumSystemVersion
#                      (actool's --minimum-deployment-target),
#   app-icon           is polygloss (actool's --app-icon, the CFBundleIconName
#                      package-release.sh sets), and
#   assets-car-sha256  is the SHA-256 of assets.car.
# The document's hash is the SHA-256 of `shasum -a 256` output for its files,
# one line `<SHA-256 of the file's raw bytes>  ./<path in the document>` per
# file whose own name does not start with '.' (files inside a dot-directory
# count), sorted by that path in byte order (LC_ALL=C). Raw bytes:
# .gitattributes keeps git from converting the document's line endings.
# POLYGLOSS_ICON_DIR (default packaging/) holds polygloss.icon and the three
# outputs; tests point it elsewhere.
#
# Rendering needs rsvg-convert (librsvg; `brew install librsvg`) or
# ImageMagick's `magick`, and the system's iconutil; compiling assets.car
# needs Xcode 26 or later. A failed run changes none of the outputs.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
svg="$repo_root/assets/icons/polygloss.svg"
info_plist="$repo_root/packaging/Info.plist"
dir="${POLYGLOSS_ICON_DIR:-$repo_root/packaging}"
doc="$dir/polygloss.icon"
car="$dir/assets.car"
manifest="$dir/assets.car.json"
app_icon=polygloss

die() {
  echo "make-icon: $*" >&2
  exit 1
}
# A path for messages: repo-relative inside the repo.
show() { echo "${1#"$repo_root"/}"; }

mode=default
case "$#:${1-}" in
  0:) ;;
  1:--check) mode=check ;;
  1:--force) mode=force ;;
  *)
    echo "usage: scripts/make-icon.sh [--check | --force]" >&2
    exit 2
    ;;
esac
[ -d "$doc" ] || die "no Icon Composer document at $(show "$doc")"
min_macos="$(plutil -extract LSMinimumSystemVersion raw -o - "$info_plist")" ||
  die "no LSMinimumSystemVersion in $(show "$info_plist")"

doc_sha256() {
  (cd "$doc" && find . -type f ! -name '.*' -print0 | LC_ALL=C sort -z |
    xargs -0 shasum -a 256) | shasum -a 256 | cut -d ' ' -f 1
}
sha256() { shasum -a 256 <"$1" | cut -d ' ' -f 1; }
# One manifest value, or nothing when it has none.
recorded() {
  local value
  value="$(plutil -extract "$1" raw -o - "$manifest" 2>/dev/null)" && printf '%s' "$value"
  return 0
}
# Why assets.car is stale; nothing when it is current.
staleness() {
  if [ ! -f "$manifest" ]; then
    echo "no $(show "$manifest")"
  elif [ ! -f "$car" ]; then
    echo "no $(show "$car")"
  elif [ "$(recorded source-sha256)" != "$(doc_sha256)" ]; then
    echo "$(show "$doc") changed since it was compiled"
  elif [ "$(recorded minimum-macos)" != "$min_macos" ]; then
    echo "it was compiled for macOS '$(recorded minimum-macos)', not $(show "$info_plist")'s $min_macos"
  elif [ "$(recorded app-icon)" != "$app_icon" ]; then
    echo "its icon is '$(recorded app-icon)', not $app_icon"
  elif [ "$(recorded assets-car-sha256)" != "$(sha256 "$car")" ]; then
    echo "it is not the file $(show "$manifest") records"
  fi
}

stale="$(staleness)"
if [ "$mode" = check ]; then
  [ -z "$stale" ] || die "$(show "$car") is stale: $stale; run scripts/make-icon.sh"
  exit 0
fi
compile=1
if [ -n "$stale" ]; then
  echo "make-icon: $(show "$car") is stale: $stale"
elif [ "$mode" = default ]; then
  compile=0
  echo "make-icon: keeping $(show "$car"), current for $(show "$doc") (--force recompiles it)"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if [ "$compile" = 1 ]; then
  xcrun actool --version >"$work/actool.plist" 2>"$work/actool.log" ||
    die "needs Xcode 26 or later: $(cat "$work/actool.log")"
  actool_info() {
    /usr/libexec/PlistBuddy -c "Print :com.apple.actool.version:$1" "$work/actool.plist"
  }
  actool_version="$(actool_info short-bundle-version)"
  [ "${actool_version%%.*}" -ge 26 ] ||
    die "actool $actool_version cannot compile Icon Composer documents: needs Xcode 26 or later"
fi

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
iconutil --convert icns --output "$work/icon.icns" "$iconset"

if [ "$compile" = 1 ]; then
  mkdir "$work/car"
  if ! xcrun actool "$doc" --compile "$work/car" --platform macosx \
    --minimum-deployment-target "$min_macos" --app-icon "$app_icon" \
    --output-partial-info-plist "$work/car/partial.plist" >"$work/actool.log" 2>&1 ||
    [ ! -f "$work/car/Assets.car" ]; then
    sed 's/^/  actool: /' "$work/actool.log" >&2
    die "actool made no Assets.car from $(show "$doc")"
  fi
  cat >"$work/assets.car.json" <<EOF
{
  "source": "polygloss.icon",
  "source-sha256": "$(doc_sha256)",
  "minimum-macos": "$min_macos",
  "app-icon": "$app_icon",
  "assets-car-sha256": "$(sha256 "$work/car/Assets.car")",
  "actool": "$actool_version ($(actool_info bundle-version))"
}
EOF
fi

# Every output moves into place only once all are made; the manifest last.
mv -f "$work/icon.icns" "$dir/icon.icns"
echo "make-icon: wrote $(show "$dir/icon.icns")"
if [ "$compile" = 1 ]; then
  mv -f "$work/car/Assets.car" "$car"
  mv -f "$work/assets.car.json" "$manifest"
  echo "make-icon: wrote $(show "$car") and $(show "$manifest")"
fi
