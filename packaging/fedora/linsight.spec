# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only

Name:           linsight
Version:        1.4.0
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

%build
cargo build --workspace --release --locked

%check
cargo test --workspace --release --locked

%install
install -Dm755 target/release/linsight     %{buildroot}%{_bindir}/linsight
install -Dm755 target/release/linsightd    %{buildroot}%{_bindir}/linsightd
install -Dm755 target/release/linsight-cli %{buildroot}%{_bindir}/linsight-cli
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
* Fri May 29 2026 VisorCraft LLC <support@visorcraft.io> - 1.4.0-1
- v1.4.0 release. Theme-aware buttons and dropdowns across the app;
  ThemedButton / ThemedComboBox shared components; dark-theme dropdown
  readability fixes; About page rework; sidebar hover fix.
* Sun May 25 2026 VisorCraft LLC <support@visorcraft.io> - 0.1.0-1
- Initial preview release.
