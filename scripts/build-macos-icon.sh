#!/usr/bin/env bash
# Render the existing mark on the macOS icon canvas; leave other platforms unchanged.
set -euo pipefail
cd "$(dirname "$0")/../apps/desktop/icons"
for size in 16 32 128 256 512; do
  for density in 1 2; do
    canvas=$((size * density))
    artwork=$(((canvas * 82 + 50) / 100))
    suffix=
    if [[ $density == 2 ]]; then suffix=@2x; fi
    magick icon-512.png -resize "${artwork}x${artwork}" -background none \
      -gravity center -extent "${canvas}x${canvas}" "icon.iconset/icon_${size}x${size}${suffix}.png"
  done
done
cp icon.iconset/icon_32x32@2x.png icon.iconset/icon_64x64.png
iconutil --convert icns --output icon.icns icon.iconset
