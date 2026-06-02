# Fedora / RHEL RPM Packaging

`linsight.spec` builds an `linsight` package for the Fedora / RHEL family
(Fedora 40+, CentOS Stream 9+, AlmaLinux 9+, Rocky Linux 9+) - any RPM
distribution that ships Qt 6 and KF6 Kirigami in the system repositories.

## Qt private API and why this needs a container on non-Fedora hosts

cxx-qt-build runs `qmlcachegen` in full AOT-to-C++ mode, which links
against Qt's private API. Private symbols are pinned to the exact Qt
minor version they were compiled against. An RPM produced on Arch /
CachyOS (Qt 6.11) will not install on Fedora 44 (Qt 6.9) - dnf rejects
it with `nothing provides libQt6Qml.so.6(Qt_6_PRIVATE_API)`. To get a
Fedora-installable RPM from a non-Fedora host, use the container path
below.

## Build (any host with podman, targets Fedora 44)

From the repo root:

```sh
just fedora-pkg
```

Or, equivalently:

```sh
bash packaging/fedora/build-in-container.sh
```

First run builds the image (~2-3 min on a reasonable connection).
Subsequent runs reuse it - pass `--rebuild-image` after editing the
`Containerfile.fedora44` or to pick up newer Fedora base updates.

The finished RPM lands under
`packaging/fedora/_rpmbuild-fedora44/RPMS/x86_64/`. Test it by passing
it to a Fedora 44 container:

```sh
podman run --rm -v ./packaging/fedora/_rpmbuild-fedora44/RPMS/x86_64:/rpms:ro \
    fedora:44 \
    dnf install -y /rpms/linsight-*.rpm
```

The container path passes `--nocheck` to `rpmbuild`, skipping the
spec's `%check` step. The test suite needs real GPU / sensor hardware
to pass (NVML, xe, NVMe sensors) which the container doesn't have.
Run the tests on a real host with `just test` separately.

Cargo registry and target dir caches persist between runs in named
podman volumes (`linsight-fedora44-cargo`, `linsight-fedora44-target`),
so iterative rebuilds are incremental and do not contaminate the
host's own `target/`. To reclaim disk:

```sh
podman volume rm linsight-fedora44-cargo linsight-fedora44-target
```

## Build (Fedora host)

From this directory:

```sh
sudo dnf install rpm-build cargo rust clang \
                 qt6-qtbase-devel qt6-qtbase-private-devel \
                 qt6-qtdeclarative-devel qt6-qtdeclarative-private-devel \
                 qt6-qttools-devel \
                 kf6-kirigami-devel sqlite-devel pkgconf-pkg-config

# Stage a source tarball matching the spec's Source0 expectation
# (prefix/name must match the workspace version in Cargo.toml).
ver=$(grep -m1 '^version' ../../Cargo.toml | cut -d'"' -f2)
( cd ../.. && git archive --format=tar.gz \
    --prefix=linsight-$ver/ --output=packaging/fedora/linsight-$ver.tar.gz HEAD )

rpmbuild --define "_topdir $(pwd)/_rpmbuild" \
         --define "_sourcedir $(pwd)" \
         -bb linsight.spec
```

The resulting `linsight-<version>-1.<dist>.x86_64.rpm` lands under
`_rpmbuild/RPMS/x86_64/`.

## Notes

- The spec ships three binaries: `linsight` (GUI), `linsightd`
  (daemon), `linsight-cli`.
- Post-install scriptlets are not currently wired in this spec; the
  package only installs files. Future revisions may add desktop /
  icon / mime cache refreshes alongside a tray-icon `%postun`.
- For COPR / Fedora-Submission, replace the `git archive` source-prep
  step with the spec's existing `Source0` GitHub URL and run
  `cargo vendor` upstream so network-free builds remain reproducible.
