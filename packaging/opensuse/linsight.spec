# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only

# Rust binaries have no usable separable debug info for openSUSE's debug
# extraction; disable the debug subpackages to avoid shipping empty ones.
%global debug_package %{nil}

Name:           linsight
Version:        1.20.3
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
* Mon Jun 29 2026 VisorCraft LLC <support@visorcraft.com> - 1.20.3-1
- Fix: Storage throughput now holds the last non-zero rate for 5 seconds
  before decaying to zero, preventing flicker on bursty NVMe I/O.

* Mon Jun 29 2026 VisorCraft LLC <support@visorcraft.com> - 1.20.2-1
- Fix: Storage throughput row was invisible because it was hidden when
  both read/write rates were zero (idle drives). Now always shown for
  disk sections. Switched to a reactive property binding for robust
  per-tick updates.

* Mon Jun 29 2026 VisorCraft LLC <support@visorcraft.com> - 1.20.1-1
- New: real-time read/write throughput (bytes per second) shown on each
  disk card header on the Storage page, auto-scaling from B/s to TiB/s.
- Fix: DesignTokens warning color referenced a non-existent
  Kirigami.Theme property, causing an undefined-QColor cascade on
  threshold-OK tile borders.
- Fix: eliminated boot-time double page incubation that produced
  spurious "not placed in the graphics scene" journal warnings.

* Mon Jun 29 2026 VisorCraft LLC <support@visorcraft.com> - 1.20.0-1
- New: NVMe drives now expose read/write operation counts (iops_read,
  iops_written) and cumulative I/O busy time (io_util_ms) on the Storage
  page. The nvme plugin already read the namespace's diskstats file but
  discarded these fields; they are now surfaced alongside bytes read/written.

* Sat Jun 27 2026 VisorCraft LLC <support@visorcraft.com> - 1.19.5-1
- Repo hygiene: stopped tracking a machine-specific .qmlls.ini that pinned an
  absolute local build path; it is now git-ignored.
- Security: .gitignore now excludes mTLS key/cert material so a locally
  generated linsight-tunnel keypair cannot be committed by accident.

* Sat Jun 27 2026 VisorCraft LLC <support@visorcraft.com> - 1.19.4-1
- Fix: clicking "Open" on a dashboard in the Gallery did nothing when it was the
  dashboard you had just left; the Gallery button now resets the page key.
- The sidebar tagline now reads "Whole system insight".

* Sat Jun 27 2026 VisorCraft LLC <support@visorcraft.com> - 1.19.3-1
- Fix unbounded GUI memory growth when QML falls behind live sample updates.
  The sample pump now coalesces pending Qt-thread updates so only the latest
  rendered frame is retained.

* Sun Jun 21 2026 VisorCraft LLC <support@visorcraft.com> - 1.19.2-1
- Fix: the GUI auto-reconnects to linsightd instead of getting stuck on
  "Disconnected". The GUI now sends a keepalive so the daemon's 30-minute idle
  timeout no longer evicts a live-but-quiet dashboard, and a supervisor
  auto-reconnects (respawning the local daemon if needed) on any drop.

* Sat Jun 20 2026 VisorCraft LLC <support@visorcraft.com> - 1.19.1-1
- Packaging release: rebuild the AppImage carrying the 1.19.0 GUI tile-rendering
  fixes; verified rendering under the AppImage's bundled Qt 6.4. No source
  changes since 1.19.0.

* Sat Jun 20 2026 VisorCraft LLC <support@visorcraft.com> - 1.19.0-1
- Fix GPU/storage tiles frozen on "…" on category pages and dashboards: the Qt
  pages merged per-tick value deltas by mutating a var map/array in place and
  reassigning the same reference (QML treats it as no change), so value bindings
  never re-evaluated. Each path now assigns a fresh reference.
- Fix static sensor values (VRAM/RAM/disk capacity) could stay on "…": statics
  are sampled once then parked, arriving in one delta a late page could miss;
  the GUI now re-emits sampled static tiles every delta.
- Fix GPU VRAM total tiles stuck on "…": static sensors were parked after the
  first scheduler tick even when their sample was dropped by the bounded sampler
  at cold start. Park on successful sample only so a dropped/errored static
  reading retries on the next due tick.

* Wed Jun 17 2026 VisorCraft LLC <support@visorcraft.com> - 1.18.0-1
- Dependency maintenance: upgrade the workspace crate tree (stabby 72,
  rusqlite 0.40, toml 1.1, evalexpr 13, rcgen 0.14, criterion 0.8, and others).
  evalexpr relicensed to AGPL-3.0-only (permitted in a GPL-3.0 project via GPLv3
  section 13). Regenerate and reconcile third-party license notices.

* Wed Jun 17 2026 VisorCraft LLC <support@visorcraft.com> - 1.17.1-1
- Fix packaged GUI failing to launch (no window): QML pages imported shared JS
  helpers via an absolute qrc:/qml/Shared.js URL the older Qt bundled in the
  AppImage could not resolve. Import them relatively.

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
- v1.7.1 release. GitHub Actions CI + multi-format release automation;
  packaging fixes and launcher icons in the RPM/DEB packages.
* Mon Jun 01 2026 VisorCraft LLC <support@visorcraft.com> - 1.7.0-1
- v1.7.0 release. Plugin panic isolation (ABI v6) and audit-driven
  security hardening.
* Sun May 31 2026 VisorCraft LLC <support@visorcraft.com> - 1.6.0-1
- v1.6.0 release. Storage mount nesting (collapsible cards), GPU VRAM
  ordering and unified sensor naming, and inode-sensor suppression for
  filesystems that don't report inodes.
