#!/usr/bin/env bash
# Renders the app icon: an SVG (default assets/icons/polygloss.svg) into an
# .icns (default packaging/icon.icns, committed) with every size macOS uses,
# 16 to 1024 px (plan T5.1). Rerun it after editing the SVG.
#
#   scripts/make-icon.sh [<icon.svg> [<icon.icns>]]
#
# Needs rsvg-convert (librsvg; `brew install librsvg`) or ImageMagick's
# `magick`, plus the system's iconutil.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
svg="${1:-$repo_root/assets/icons/polygloss.svg}"
icns="${2:-$repo_root/packaging/icon.icns}"

die() {
  echo "make-icon: $*" >&2
  exit 1
}

[ "$#" -le 2 ] || {
  echo "usage: scripts/make-icon.sh [<icon.svg> [<icon.icns>]]" >&2
  exit 2
}
[ -f "$svg" ] || die "no such svg: $svg"

if command -v rsvg-convert >/dev/null; then
  render() { rsvg-convert --width "$1" --height "$1" --output "$2" "$svg"; }
elif command -v magick >/dev/null; then
  render() { magick -background none -density 384 "$svg" -resize "$1x$1" "$2"; }
else
  die "needs rsvg-convert (brew install librsvg) or ImageMagick's magick"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
iconset="$work/icon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
  render "$size" "$iconset/icon_${size}x${size}.png"
  render "$((size * 2))" "$iconset/icon_${size}x${size}@2x.png"
done
mkdir -p "$(dirname "$icns")"
iconutil --convert icns --output "$icns" "$iconset"
echo "make-icon: wrote $icns"
