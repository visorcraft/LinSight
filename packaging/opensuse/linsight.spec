# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only

# Rust binaries have no usable separable debug info for openSUSE's debug
# extraction; disable the debug subpackages to avoid shipping empty ones.
%global debug_package %{nil}

Name:           linsight
Version:        1.7.0
Release:        0
Summary:        Fast multi-GPU Linux system monitor
License:        GPL-3.0-only
Group:          System/Monitoring
URL:            https://github.com/visorcraft/linsight
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.95
BuildRequires:  cargo
BuildRequires:  cmake(Qt6Core)
BuildRequires:  cmake(Qt6Quick)
BuildRequires:  cmake(KF6Kirigami)
BuildRequires:  pkgconfig(sqlite3)
BuildRequires:  clang

%description
LinSight — a Linux-native multi-GPU system monitor with a runtime plugin
system. See the Fedora spec for full feature notes; the package
structure is identical to it.

%prep
%autosetup

# Pin CARGO_TARGET_DIR to an absolute path so %build and %install agree
# regardless of the rpm build-directory layout (see Fedora spec note).
%global cargo_target %{_builddir}/_cargo_target

%build
export CARGO_TARGET_DIR=%{cargo_target}
cargo build --workspace --release --locked

%install
# Under modern rpm's build layout %install's working directory is not
# reliably the extracted source root, so cd into it explicitly.
cd %{_builddir}/%{name}-%{version}
install -Dm755 %{cargo_target}/release/linsight     %{buildroot}%{_bindir}/linsight
install -Dm755 %{cargo_target}/release/linsightd    %{buildroot}%{_bindir}/linsightd
install -Dm755 %{cargo_target}/release/linsight-cli %{buildroot}%{_bindir}/linsight-cli
install -Dm644 packaging/io.visorcraft.LinSight.desktop \
    %{buildroot}%{_datadir}/applications/io.visorcraft.LinSight.desktop
install -Dm644 packaging/io.visorcraft.LinSight.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/io.visorcraft.LinSight.metainfo.xml
install -Dm644 packaging/systemd/linsight.service \
    %{buildroot}%{_userunitdir}/linsight.service
install -d %{buildroot}%{_libdir}/linsight/plugins

%files
%license LICENSE
%doc README.md
%{_bindir}/linsight
%{_bindir}/linsightd
%{_bindir}/linsight-cli
%{_datadir}/applications/io.visorcraft.LinSight.desktop
%{_datadir}/metainfo/io.visorcraft.LinSight.metainfo.xml
%{_userunitdir}/linsight.service
%dir %{_libdir}/linsight/plugins

%changelog
* Mon Jun 01 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.0-1
- v1.7.0 release. Plugin panic isolation (ABI v6) and audit-driven
  security hardening.
* Sun May 31 2026 VisorCraft LLC <support@visorcraft.com> - 1.6.0-1
- v1.6.0 release. Storage mount nesting (collapsible cards), GPU VRAM
  ordering and unified sensor naming, and inode-sensor suppression for
  filesystems that don't report inodes.
