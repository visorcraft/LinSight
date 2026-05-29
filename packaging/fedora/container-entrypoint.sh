#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Entrypoint that runs inside the Fedora 44 build container.
# Expects:
#   /src              read-only bind mount of the linsight repo root
#   /output           writable bind mount where the finished RPM lands
#   /home/builder/.cargo (volume)        cargo cache
#   /home/builder/target-cache (volume)  container-private cargo target dir
#
# Reads the workspace version from /src/Cargo.toml at run time so this
# script does not need to be touched on version bumps.
#
# We pass --nocheck to rpmbuild: the spec's %check step runs the full
# cargo test suite, which in a container without GPU/sensor hardware
# would fail half the suite (NVML, xe sensors, etc.). Run those tests
# on a real host instead.

set -euo pipefail

# Read the workspace package version from Cargo.toml.
version="$(awk -F'"' '
    /^\[workspace\.package\]/ { in_section = 1; next }
    in_section && /^\[/        { exit }
    in_section && $1 ~ /^version[[:space:]]*=/ { print $2; exit }
' /src/Cargo.toml)"

if [ -z "${version:-}" ]; then
    echo "ERROR: could not parse workspace version from /src/Cargo.toml" >&2
    exit 1
fi

echo "==> Building linsight ${version} RPM inside Fedora 44 container"
echo "    Qt version on this host:"
rpm -q qt6-qtbase qt6-qtdeclarative | sed 's/^/      /'

# Stage a clean copy of the source. Bind mount is read-only; rpmbuild
# needs to write to the spec's _sourcedir and we want to be sure the
# host tree is never modified.
work="$(mktemp -d /tmp/linsight-build.XXXXXX)"
trap 'rm -rf "$work"' EXIT
cp -a /src/. "$work/repo"

# Point cargo at the container-private target dir so host builds and
# container builds don't fight over the same compiled artefacts.
export CARGO_TARGET_DIR=/home/builder/target-cache
export CARGO_HOME=/home/builder/.cargo

cd "$work/repo"

# Spec uses %autosetup which expects a tarball at <sourcedir>/linsight-<version>.tar.gz
# unpacking to a top-level linsight-<version>/ dir.
git_available=true
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git_available=false
fi

if $git_available; then
    git archive --format=tar.gz \
        --prefix="linsight-${version}/" \
        --output="packaging/fedora/linsight-${version}.tar.gz" HEAD
else
    # Fallback for source trees mounted without .git (rare). Tar the
    # working copy minus build/output artefacts.
    tar --transform "s|^\.|linsight-${version}|" \
        --exclude=./target --exclude=./packaging/fedora/_rpmbuild \
        --exclude=./.git \
        -czf "packaging/fedora/linsight-${version}.tar.gz" .
fi

cd packaging/fedora

# Sync the spec's Version field to Cargo.toml if they've drifted. The
# spec ships with 1.0.0; if Cargo.toml is ahead, use Cargo.toml.
spec_version="$(awk '/^Version:/ {print $2; exit}' linsight.spec)"
if [ "$spec_version" != "$version" ]; then
    echo "    Note: linsight.spec Version (${spec_version}) lags Cargo.toml (${version}); using Cargo.toml"
    sed -i "s/^Version:.*/Version:        ${version}/" linsight.spec
fi

# --nocheck skips %check (tests). See header comment.
rpmbuild --define "_topdir $(pwd)/_rpmbuild" \
         --define "_sourcedir $(pwd)" \
         --nocheck \
         -bb linsight.spec

# Copy the RPMs into the host-mounted output dir.
mkdir -p /output
find _rpmbuild/RPMS -type f -name '*.rpm' -exec cp -v {} /output/ \;

echo
echo "==> Done. RPM(s) copied to /output (host-mounted):"
ls -l /output/*.rpm 2>/dev/null || echo "    (no RPMs found - build may have failed)"
