# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only

Name:           linsight
Version:        1.4.0
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

%build
cargo build --workspace --release --locked

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
