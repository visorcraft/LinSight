# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only

# Rust binaries have no usable separable debug info for openSUSE's debug
# extraction; disable the debug subpackages to avoid shipping empty ones.
%global debug_package %{nil}

Name:           linsight
Version:        1.9.0
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
install -Dm644 packaging/com.visorcraft.LinSight.desktop \
    %{buildroot}%{_datadir}/applications/com.visorcraft.LinSight.desktop
install -Dm644 packaging/com.visorcraft.LinSight.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/com.visorcraft.LinSight.metainfo.xml
install -Dm644 packaging/systemd/linsight.service \
    %{buildroot}%{_userunitdir}/linsight.service
install -Dm644 packaging/icons/scalable/apps/com.visorcraft.LinSight.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/com.visorcraft.LinSight.svg
for _s in 16 24 32 48 64 96 128 192 256 512; do
  install -Dm644 packaging/icons/${_s}x${_s}/apps/com.visorcraft.LinSight.png \
    %{buildroot}%{_datadir}/icons/hicolor/${_s}x${_s}/apps/com.visorcraft.LinSight.png
done
install -d %{buildroot}%{_libdir}/linsight/plugins

%files
%license LICENSE
%doc README.md
%{_bindir}/linsight
%{_bindir}/linsightd
%{_bindir}/linsight-cli
%{_datadir}/applications/com.visorcraft.LinSight.desktop
%{_datadir}/metainfo/com.visorcraft.LinSight.metainfo.xml
%{_userunitdir}/linsight.service
%{_datadir}/icons/hicolor/*/apps/com.visorcraft.LinSight.*
%dir %{_libdir}/linsight/plugins

%changelog
* Thu Jun 04 2026 VisorCraft LLC <support@visorcraft.com> - 1.9.0-1
- Harden daemon subscriptions, history, Prometheus, plugin loading, dashboard
  persistence, CLI JSON/socket defaults, tunnel shutdown, and CPU/memory sensor
  sampling.

* Tue Jun 02 2026 VisorCraft LLC <support@visorcraft.com> - 1.8.0-1
- New application icon across all packaged sizes and the in-app pages.
- Third-party license credits now exclude first-party crates (publish = false),
  so the Credits page lists only genuine dependencies.
- Documentation cleanup.

* Tue Jun 02 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.3-1
- v1.7.3 release. Rename the application ID from io.visorcraft.LinSight to
  com.visorcraft.LinSight (desktop entry, AppStream metainfo, icons, Flatpak
  app-id, GUI window id) to match the visorcraft.com domain, and the sensor
  plugin-ids from io.visorcraft.linsight.* to com.visorcraft.linsight.*.
  NOTE: a pinned launcher must be re-pinned after upgrading.
* Tue Jun 02 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.2-1
- v1.7.2 release. Rename the bundled third-party credits to
  docs/third-party-notices.md and regenerate against the current dependency
  set; refresh translation catalogs.
* Tue Jun 02 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.1-1
- v1.7.1 release. GitHub Actions CI + multi-format release automation;
  packaging fixes and launcher icons in the RPM/DEB packages.
* Mon Jun 01 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.0-1
- v1.7.0 release. Plugin panic isolation (ABI v6) and audit-driven
  security hardening.
* Sun May 31 2026 VisorCraft LLC <support@visorcraft.com> - 1.6.0-1
- v1.6.0 release. Storage mount nesting (collapsible cards), GPU VRAM
  ordering and unified sensor naming, and inode-sensor suppression for
  filesystems that don't report inodes.
