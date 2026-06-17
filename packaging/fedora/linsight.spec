# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only

# Rust release binaries carry no separable debug info that Fedora's
# automatic debuginfo/debugsource extraction can use (cargo builds into an
# out-of-tree target), so disable the debug packages — otherwise rpmbuild
# fails on an empty debugsourcefiles.list.
%global debug_package %{nil}

Name:           linsight
Version:        1.17.0
Release:        1%{?dist}
Summary:        Fast, beautiful Linux system-monitoring dashboard with multi-GPU support

License:        GPL-3.0-only
URL:            https://github.com/visorcraft/linsight
Source0:        https://github.com/visorcraft/linsight/archive/v%{version}/%{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.95
BuildRequires:  cargo
BuildRequires:  qt6-qtbase-devel
BuildRequires:  qt6-qtdeclarative-devel
BuildRequires:  qt6-qttools-devel
BuildRequires:  kf6-kirigami-devel
BuildRequires:  sqlite-devel
BuildRequires:  clang

Requires:       qt6-qtbase
Requires:       qt6-qtdeclarative
Requires:       kf6-kirigami

%description
LinSight is a Linux-native multi-GPU system monitor with a runtime plugin
system. It shows CPU, RAM, NVIDIA + Intel xe GPUs, NVMe drives, and network
interfaces on a single Overview page. Optional always-on mode adds
SQLite-backed history and a Prometheus exporter.

%prep
%autosetup

# Pin CARGO_TARGET_DIR to an absolute path. Under rpm 6.0's build-dir
# layout the relative ./target that %install assumed is not always where
# cargo writes; an absolute, macro-derived path is identical in %build,
# %check and %install regardless of the working directory.
%global cargo_target %{_builddir}/_cargo_target

%build
export CARGO_TARGET_DIR=%{cargo_target}
cargo build --workspace --release --locked

%check
export CARGO_TARGET_DIR=%{cargo_target}
cargo test --workspace --release --locked

%install
# Under rpm 6.0's build layout %install's working directory is not reliably
# the extracted source root, so cd into it explicitly; the source-relative
# install inputs (packaging/, LICENSE, README) resolve from here. Binaries
# come from the absolute %{cargo_target} regardless.
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
* Wed Jun 17 2026 VisorCraft LLC <support@visorcraft.com> - 1.17.0-1
- v1.17.0 release. Performance and UI polish: Arc-shared samples/catalogues,
  reusable serialization buffers, dirty-delta GUI tile updates, lightweight
  history records, shared alert context, stack-buffer sysfs reads, dashboard
  import in Rust, mutable daemon settings toggles, and CLI format validation.

* Tue Jun 16 2026 VisorCraft LLC <support@visorcraft.com> - 1.16.0-1
- v1.16.0 release. Harden daemon and GUI against hangs and leaks: capped,
  time-boxed sensor worker threads, async alert notifications, transport and
  client socket timeouts, idempotent subscriptions, and tunnel/Prometheus
  write-timeouts.

* Sun Jun 14 2026 VisorCraft LLC <support@visorcraft.com> - 1.15.0-1
- v1.15.0 release. Criterion benchmarks, proptest round-trips, daemon transport
  unit tests, GUI smoke in CI, plugin sandbox design groundwork.

* Fri Jun 12 2026 VisorCraft LLC <support@visorcraft.com> - 1.14.1-1
- v1.14.1 release. Release artifact race fix, GUI client dispatch tests,
  daemon XDG path helper refactor, and deterministic wire-size budget guard.

* Fri Jun 12 2026 VisorCraft LLC <support@visorcraft.com> - 1.14.0-1
- v1.14.0 release. UI polish batch: dedicated Network page with per-interface
  throughput, persisted process-table state, copy sensor ID from any tile,
  read-only Prometheus bind hint, and refreshed i18n.

* Fri Jun 12 2026 VisorCraft LLC <support@visorcraft.com> - 1.13.0-1
- v1.13.0 release. Multi-host view: save remote hosts, switch between them
  from the sidebar, and survive reconnects with catalogue rebuild,
  subscription replay, and pump-interval replay.

* Thu Jun 11 2026 VisorCraft LLC <support@visorcraft.com> - 1.11.0-1
- Version bump to 1.11.0.

* Wed Jun 10 2026 VisorCraft LLC <support@visorcraft.com> - 1.10.0-1
- Process explorer page with sortable/filterable process table, SMART disk
  health sensors, alert event log and cooldown, sensor snapshot caches, and
  history charts with retention pruning.

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
- v1.7.1 release. GitHub Actions CI + multi-format release automation
  (tarball, Arch, deb, rpm, AppImage, Flatpak); packaging fixes and
  launcher icons in the RPM/DEB packages.
* Mon Jun 01 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.0-1
- v1.7.0 release. Plugin panic isolation (ABI v6) and audit-driven
  security hardening.
* Sun May 31 2026 VisorCraft LLC <support@visorcraft.com> - 1.6.0-1
- v1.6.0 release. Storage page nests mount points inside their physical disk
  as collapsible cards (ordered by capacity); GPU sections ordered by VRAM
  with unified "GPU VRAM" sensor naming; inode sensors skipped for
  filesystems that don't report them.
* Sat May 30 2026 VisorCraft LLC <support@visorcraft.com> - 1.5.0-1
- v1.5.0 release. Dynamic plugin config delivery plus container and
  socket statistics sensors.
* Fri May 29 2026 VisorCraft LLC <support@visorcraft.com> - 1.4.1-1
- v1.4.1 release. Correct the company domain to visorcraft.com (Qt
  organizationDomain + packaging contact fields); packaging recipes
  build end-to-end from a clean checkout.
* Fri May 29 2026 VisorCraft LLC <support@visorcraft.com> - 1.4.0-1
- v1.4.0 release. Theme-aware buttons and dropdowns across the app;
  ThemedButton / ThemedComboBox shared components; dark-theme dropdown
  readability fixes; About page rework; sidebar hover fix.
* Mon May 25 2026 VisorCraft LLC <support@visorcraft.com> - 0.1.0-1
- Initial preview release.
