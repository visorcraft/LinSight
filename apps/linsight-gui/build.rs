// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

use cxx_qt_build::{CxxQtBuilder, QmlModule};
use qt_build_utils::{QResource, QResourceFile, QResources};

fn main() {
    println!("cargo:rerun-if-changed=src/translator.cpp");
    println!("cargo:rerun-if-changed=src/screenshot.cpp");
    println!("cargo:rerun-if-changed=src/icon_theme.cpp");
    println!("cargo:rerun-if-changed=i18n/linsight_en.qm");
    println!("cargo:rerun-if-changed=i18n/linsight_de.qm");
    println!("cargo:rerun-if-changed=i18n/linsight_ja.qm");
    println!("cargo:rerun-if-changed=../../LICENSE");
    println!("cargo:rerun-if-changed=../../CREDITS.md");
    println!("cargo:rerun-if-changed=../../docs/third-party-notices.md");
    println!("cargo:rerun-if-changed=resources/linsight.svg");
    println!("cargo:rerun-if-changed=resources/linsight-32.png");
    println!("cargo:rerun-if-changed=resources/linsight-64.png");
    println!("cargo:rerun-if-changed=resources/linsight-128.png");
    println!("cargo:rerun-if-changed=resources/linsight-256.png");
    println!("cargo:rerun-if-changed=resources/linsight-512.png");

    let builder = CxxQtBuilder::new_qml_module(
        QmlModule::new("com.visorcraft.LinSight").version(1, 0).qml_files([
            "qml/Main.qml",
            "qml/DashWindow.qml",
            "qml/OverviewPage.qml",
            "qml/CategoryPage.qml",
            "qml/ProcessesPage.qml",
            "qml/SensorTile.qml",
            "qml/StorageSectionView.qml",
            "qml/DesignTokens.qml",
            "qml/NavItem.qml",
            "qml/ThemedButton.qml",
            "qml/ThemedComboBox.qml",
            "qml/SettingsPage.qml",
            "qml/SettingsCard.qml",
            "qml/AboutPage.qml",
            "qml/LicensesPage.qml",
            "qml/CreditsPage.qml",
            "qml/GplLicenseDialog.qml",
            "qml/CanvasEditorPage.qml",
            "qml/ThemePicker.qml",
            "qml/StartPagePicker.qml",
            "qml/NewDashboardDialog.qml",
            "qml/DashboardViewPage.qml",
            "qml/HardwarePage.qml",
            "qml/ThemedTextField.qml",
            "qml/AlertsPage.qml",
            "qml/HistoryChart.qml",
            "qml/HistoryDialog.qml",
            "qml/NetworkPage.qml",
            "qml/NetworkCard.qml",
            "qml/NetworkMetric.qml",
        ]),
    )
    .file("src/qobjects/overview_model.rs")
    .file("src/qobjects/preferences_model.rs")
    .file("src/qobjects/dashboards_model.rs")
    .file("src/qobjects/hardware_model.rs")
    .file("src/qobjects/history_model.rs")
    .file("src/qobjects/hosts_model.rs")
    .file("src/qobjects/alert_model.rs")
    .qrc_resources(
        QResources::new().resource(
            QResource::new()
                .file(QResourceFile::new("i18n/linsight_en.qm").alias("i18n/linsight_en.qm"))
                .file(QResourceFile::new("i18n/linsight_de.qm").alias("i18n/linsight_de.qm"))
                .file(QResourceFile::new("i18n/linsight_ja.qm").alias("i18n/linsight_ja.qm"))
                // Bundle LICENSE + auto-generated third-party credits so the
                // About / Licenses / Credits pages can XHR them at runtime.
                .file(QResourceFile::new("../../LICENSE").alias("docs/LICENSE"))
                .file(
                    QResourceFile::new("../../docs/third-party-notices.md")
                        .alias("docs/third-party-notices.md"),
                )
                // Brand icon set. Main.qml's sidebar header references
                // `resources/linsight-128.png`; the rest are included for
                // window-manager / taskbar usage at native densities.
                .file(QResourceFile::new("resources/linsight.svg").alias("resources/linsight.svg"))
                .file(
                    QResourceFile::new("resources/linsight-32.png")
                        .alias("resources/linsight-32.png"),
                )
                .file(
                    QResourceFile::new("resources/linsight-64.png")
                        .alias("resources/linsight-64.png"),
                )
                .file(
                    QResourceFile::new("resources/linsight-128.png")
                        .alias("resources/linsight-128.png"),
                )
                .file(
                    QResourceFile::new("resources/linsight-256.png")
                        .alias("resources/linsight-256.png"),
                )
                .file(
                    QResourceFile::new("resources/linsight-512.png")
                        .alias("resources/linsight-512.png"),
                ),
        ),
    );

    // screenshot.cpp uses QQuickWindow::grabWindow to render the QML
    // scene to a PNG bypassing the compositor — needed because Wayland
    // serves stale cached surfaces for unfocused windows.
    let builder = builder.qt_module("Quick");

    let builder = unsafe {
        builder.cc_builder(|cc| {
            cc.file("src/translator.cpp");
            cc.file("src/screenshot.cpp");
            cc.file("src/icon_theme.cpp");
            // GCC 16 added `-Wsfinae-incomplete`, which fires whenever a
            // class is defined after having previously appeared in a
            // SFINAE context as incomplete. Qt 6's `QChar` trips it via
            // a libstdc++ `unordered_map` / `range_access` metaprogramming
            // probe that runs before the class definition is complete.
            // Every cxx-qt-generated `.cxx.cpp` shim transitively
            // includes `<QStringList>` so the warning fires per qobject —
            // hundreds of lines of build-log noise from a system-header
            // interaction we can't fix upstream. Silence it for our
            // C++ compilation; older compilers / clang don't know the
            // flag and `flag_if_supported` drops it silently.
            cc.flag_if_supported("-Wno-sfinae-incomplete");
        })
    };

    builder.build();
}
