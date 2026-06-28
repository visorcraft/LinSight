#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Regenerate every derived copy of the LinSight app icon from the
# canonical master at assets/LinSight.svg.
#
# Sources of truth (committed, hand-authored):
#   assets/LinSight.svg    — the app icon
#   assets/social-card.svg — the GitHub social-preview banner (embeds the icon)
#
# Everything else this script writes is a derived artifact and must trace
# back to those two files. Re-run after editing either source.
#
# Requires: rsvg-convert (librsvg) and magick (ImageMagick 7).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MASTER="assets/LinSight.svg"
BANNER="assets/social-card.svg"
APPID="com.visorcraft.LinSight"

for tool in rsvg-convert magick; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: '$tool' not found in PATH" >&2; exit 1; }
done

echo "==> FreeDesktop hicolor PNGs (packaging/icons)"
for size in 16 24 32 48 64 96 128 192 256 512; do
  out="packaging/icons/${size}x${size}/apps/${APPID}.png"
  install -d "$(dirname "$out")"
  rsvg-convert -w "$size" -h "$size" "$MASTER" -o "$out"
  echo "    $out"
done
install -Dm644 "$MASTER" "packaging/icons/scalable/apps/${APPID}.svg"
echo "    packaging/icons/scalable/apps/${APPID}.svg"

echo "==> GUI embedded resources (apps/linsight-gui/resources)"
for size in 32 64 128 256 512; do
  out="apps/linsight-gui/resources/linsight-${size}.png"
  rsvg-convert -w "$size" -h "$size" "$MASTER" -o "$out"
  echo "    $out"
done
install -Dm644 "$MASTER" "apps/linsight-gui/resources/linsight.svg"
echo "    apps/linsight-gui/resources/linsight.svg"

echo "==> Master raster (assets/LinSight.png, 1024x1024)"
rsvg-convert -w 1024 -h 1024 "$MASTER" -o "assets/LinSight.png"

echo "==> Multi-resolution Windows icon (assets/LinSight.ico)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for size in 16 32 48 64 128 256; do
  rsvg-convert -w "$size" -h "$size" "$MASTER" -o "$tmp/icon-$size.png"
done
magick "$tmp"/icon-16.png "$tmp"/icon-32.png "$tmp"/icon-48.png \
       "$tmp"/icon-64.png "$tmp"/icon-128.png "$tmp"/icon-256.png \
       "assets/LinSight.ico"

echo "==> GitHub social preview (assets/social-1024x512.png, 1024x512)"
rsvg-convert -w 1024 -h 512 "$BANNER" -o "assets/social-1024x512.png"

echo "==> done"
