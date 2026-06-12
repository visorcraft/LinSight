# SPDX-FileCopyrightText: 2026 VisorCraft LLC
# SPDX-License-Identifier: GPL-3.0-only
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Format every crate.
fmt:
    cargo fmt --all

# Check formatting without mutating the worktree.
fmt-check:
    cargo fmt --all -- --check

# Cargo build (debug) for the entire workspace.
build:
    cargo build --workspace

# Cargo build (release) for the entire workspace.
build-release:
    cargo build --workspace --release

# Cargo build (release) tuned for x86_64-v3 (CachyOS / modern systems).
build-release-v3:
    RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --workspace --release

# Run every test in the workspace.
test:
    cargo test --workspace

# Type-check without producing binaries.
check:
    cargo check --workspace

# Strict lint pass — same gate as CI.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# License + advisory check (requires `cargo install cargo-deny`).
deny:
    cargo deny --all-features check

# Recursive audit (requires `cargo install cargo-audit`).
audit:
    cargo audit

# Run the daemon standalone (useful for manual smoke tests).
run-daemon *args:
    cargo run -p linsightd -- {{args}}

# Run the CLI (passes args through).
run-cli *args:
    cargo run -p linsight-cli -- {{args}}

# Extract translatable strings from the GUI's QML files into .ts catalogs.
# Run after adding/changing any qsTr() call. Requires `lupdate6` from
# `qt6-tools`.
#
# All QML files that contain qsTr() must be listed here; lupdate6 only
# scans the explicit list. Anything omitted will not appear in the .ts
# catalogs and will display as English regardless of locale.
i18n-extract:
    cd apps/linsight-gui && lupdate6 \
        qml/Main.qml \
        qml/OverviewPage.qml \
        qml/CanvasEditorPage.qml \
        qml/CategoryPage.qml \
        qml/SensorTile.qml \
        qml/ProcessesPage.qml \
        qml/SettingsPage.qml \
        qml/AboutPage.qml \
        qml/LicensesPage.qml \
        qml/CreditsPage.qml \
        qml/DashWindow.qml \
        qml/GplLicenseDialog.qml \
        qml/ThemePicker.qml \
        qml/StartPagePicker.qml \
        qml/NewDashboardDialog.qml \
        qml/DashboardViewPage.qml \
        qml/HardwarePage.qml \
        qml/AlertsPage.qml \
        qml/HistoryChart.qml \
        qml/HistoryDialog.qml \
        qml/NetworkPage.qml \
        qml/NetworkCard.qml \
        qml/NetworkMetric.qml \
        -ts i18n/linsight_en.ts i18n/linsight_de.ts i18n/linsight_ja.ts

# Compile .ts catalogs to runtime-loadable .qm files. Run after
# completing translations. Requires `lrelease6` from `qt6-tools`.
i18n-compile:
    cd apps/linsight-gui && lrelease6 \
        i18n/linsight_en.ts i18n/linsight_de.ts i18n/linsight_ja.ts

# Convenience target — everything CI does.
ci: fmt-check lint test
    @echo "ci preflight passed"

# Headless GUI boot smoke. Builds linsight release, runs under xvfb-run,
# asserts the daemon handshake completes within 12s. Skipped when
# xvfb-run isn't installed. Run after any QML / cxx-qt change.
gui-smoke:
    ./scripts/gui_smoke.sh

# Build an Arch package via `makepkg -si` from packaging/arch/.
arch-pkg:
    cd packaging/arch && makepkg -si --noconfirm

# Build the x86_64-v3 Arch variant (CachyOS-friendly).
arch-pkg-v3:
    cd packaging/arch-v3 && makepkg -si --noconfirm

# The container path matters because cxx-qt-build's QML AOT compiler
# links against Qt's private API, which is pinned to the exact Qt
# minor version. An RPM built on Arch / CachyOS (Qt 6.11) fails to
# install on Fedora 44 (Qt 6.9). See packaging/fedora/Containerfile.fedora44.
# Build the Fedora 44 .rpm inside a podman container (Qt 6.9 matched).
fedora-pkg:
    bash packaging/fedora/build-in-container.sh

# Build the Flatpak. Vendors crates first so the sandbox can compile offline.
flatpak: flatpak-vendor
    cd packaging/flatpak && flatpak-builder --user --force-clean \
        --install --install-deps-from=flathub \
        build com.visorcraft.LinSight.yml

# Vendor all cargo dependencies for the Flatpak sandboxed build.
flatpak-vendor:
    rm -rf packaging/flatpak/vendor
    cargo vendor --locked packaging/flatpak/vendor
    tar -C packaging/flatpak -czf packaging/flatpak/vendor.tar.gz vendor/

# Build the AppImage via appimage-builder. Injects the workspace version
# dynamically. Requires `appimage-builder` on PATH.
appimage:
    bash scripts/build_appimage.sh

# Regenerate the bundled third-party notices markdown from Cargo.lock.
# The Credits page in the GUI points users at this command; runs
# `cargo about generate about.hbs` and normalizes to LF (cargo-about
# emits CRLF, which the repo stores as LF). Requires `cargo install
# cargo-about`.
credits:
    cargo about generate about.hbs | sed 's/\r$//' > docs/third-party-notices.md

# Full pre-release gate.
preflight: ci deny audit
    @echo "preflight passed"
