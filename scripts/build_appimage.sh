#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Build the LinSight AppImage via appimage-builder. The recipe's
# app_info.version is overwritten from the workspace Cargo.toml at build
# time so the produced AppImage is always versioned correctly, even if
# the committed recipe value lags. Requires appimage-builder on PATH.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="$(awk -F'"' '
    /^\[workspace\.package\]/ { in_section = 1; next }
    in_section && /^\[/ { exit }
    in_section && $1 ~ /^version[[:space:]]*=/ { print $2; exit }
' Cargo.toml)"
[[ -n "$version" ]] || { echo "ERROR: could not extract version from Cargo.toml" >&2; exit 1; }

mkdir -p target/appimage
# Rewrite the 4-space-indented app_info.version line (the top-level
# recipe `version: 1` is at column 0, so it is untouched).
sed "s/^    version: .*/    version: ${version}/" \
    packaging/appimage/AppImageBuilder.yml \
    > target/appimage/AppImageBuilder.yml

rm -rf AppDir
appimage-builder --recipe target/appimage/AppImageBuilder.yml --skip-test
echo "AppImage built for version ${version}"
