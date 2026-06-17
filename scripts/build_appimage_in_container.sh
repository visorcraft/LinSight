#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  ca-certificates git curl file \
  qt6-base-dev qt6-declarative-dev qt6-declarative-dev-tools \
  qt6-tools-dev kirigami2-dev libgl1-mesa-dev \
  pkg-config clang ninja-build \
  python3-pip patchelf desktop-file-utils fakeroot zsync \
  squashfs-tools gnupg2 libglib2.0-bin debian-archive-keyring \
  gtk-update-icon-cache

pip3 install --break-system-packages appimage-builder==1.1.0

export RUSTUP_HOME=/usr/local/rustup
export CARGO_HOME=/usr/local/cargo
export HOME=/root
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --default-toolchain 1.95 --profile minimal --no-modify-path
export PATH="/usr/local/cargo/bin:$PATH"

cat > /usr/local/bin/apt-key <<'SHIM'
#!/bin/sh
if [ "$1" = "add" ]; then
  src="${2:--}"
  out="/etc/apt/trusted.gpg.d/appimage-builder-added.gpg"
  if [ "$src" = "-" ]; then cat; else cat "$src"; fi \
    | gpg --dearmor >> "$out" 2>/dev/null || true
fi
exit 0
SHIM
chmod +x /usr/local/bin/apt-key

export APPIMAGE_EXTRACT_AND_RUN=1
export QMAKE=/usr/bin/qmake6
export QT_VERSION_MAJOR=6
command -v "$QMAKE" >/dev/null || { echo "ERROR: $QMAKE not found" >&2; exit 1; }
bash scripts/build_appimage.sh
