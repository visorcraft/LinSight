#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
#
# Host-side wrapper that builds the linsight Fedora 44 RPM inside a
# podman container. Use this instead of running rpmbuild on the host
# when you need an RPM that installs on Fedora 44 - the on-host
# rpmbuild path links against whatever Qt the host has (Qt 6.11 on
# CachyOS), and Qt's AOT-compiled QML binds to private symbols that
# only exist on the matching Qt minor version. See
# Containerfile.fedora44 for the longer explanation.
#
# Output:
#   packaging/fedora/_rpmbuild-fedora44/RPMS/x86_64/linsight-<ver>-1.fc44.x86_64.rpm
#
# First run builds the image (~2-3 min). Subsequent runs reuse it.
# Pass --rebuild-image to force a fresh image build (after editing
# the Containerfile, or to pick up newer Fedora base updates).

set -euo pipefail

self_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${self_dir}/../.." && pwd)"
image_tag="linsight-rpm-fedora44"
cargo_volume="linsight-fedora44-cargo"
output_dir="${self_dir}/_rpmbuild-fedora44/RPMS/x86_64"

rebuild_image=false
for arg in "$@"; do
    case "$arg" in
        --rebuild-image) rebuild_image=true ;;
        -h|--help)
            sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

if ! command -v podman >/dev/null 2>&1; then
    echo "podman is required but not installed." >&2
    echo "Install: sudo pacman -S podman   (Arch/CachyOS)" >&2
    exit 1
fi

cd "$self_dir"

# Build the image if it doesn't exist or if --rebuild-image was passed.
if $rebuild_image || ! podman image exists "$image_tag"; then
    echo "==> Building container image (${image_tag})"
    podman build -f Containerfile.fedora44 -t "$image_tag" .
else
    echo "==> Reusing existing container image (${image_tag})"
    echo "    Pass --rebuild-image to force a rebuild after Containerfile or base-image changes."
fi

mkdir -p "$output_dir"
# The container runs as the unprivileged `builder` user, which under rootless
# podman maps to a subuid that is neither the dir's owner nor in its group.
# Make the output dir world-writable so the finished RPM can be copied out.
chmod 0777 "$output_dir"

# The workspace Cargo.toml pins qt-build-utils to a local cxx-qt fork via
# [patch.crates-io] using an absolute path *outside* this repo. The container
# only mounts /src, so find that fork's workspace root and bind-mount it
# read-only at the same path, letting `cargo --locked` resolve the patch.
patch_mount=()
fork_crate="$(sed -nE 's/^[[:space:]]*qt-build-utils[[:space:]]*=.*\bpath[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' \
    "${repo_root}/Cargo.toml" | head -1)"
if [ -n "${fork_crate:-}" ]; then
    fork_root="$fork_crate"
    while [ "$fork_root" != "/" ] && ! grep -qs '^\[workspace\]' "${fork_root}/Cargo.toml"; do
        fork_root="$(dirname "$fork_root")"
    done
    if [ -d "$fork_root" ] && [ "$fork_root" != "/" ]; then
        echo "==> Patch dependency: bind-mounting cxx-qt fork ${fork_root}"
        patch_mount=(-v "${fork_root}:${fork_root}:ro")
    else
        echo "WARNING: Cargo.toml patches qt-build-utils to ${fork_crate} but its" >&2
        echo "         workspace root was not found; the container build will fail." >&2
    fi
fi

echo "==> Building RPM"
echo "    Source : ${repo_root}"
echo "    Output : ${output_dir}"
echo

# --security-opt label=disable bypasses SELinux relabeling on the bind
# mount. CachyOS doesn't run SELinux, but the flag keeps the script
# portable to hosts that do without rewriting file labels on the
# user's repo.
podman run --rm \
    --security-opt label=disable \
    -v "${repo_root}:/src:ro" \
    "${patch_mount[@]}" \
    -v "${output_dir}:/output" \
    -v "${cargo_volume}:/home/builder/.cargo" \
    "$image_tag"

echo
echo "==> Built RPMs:"
ls -l "$output_dir"/*.rpm 2>/dev/null || {
    echo "    (none - the container build did not produce an RPM)" >&2
    exit 1
}
