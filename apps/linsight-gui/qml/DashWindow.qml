// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
//
// Secondary dashboard window, instantiated via Qt.createQmlObject from
// Main.qml's "Open new window" action. Shares the app-scope `dashModel`
// from the host window so every sample feeds every visible tile.
//
// Property name is `dashModel` to match what every page actually reads
// (OverviewPage / CategoryPage / SensorTile all destructure `dashModel`).
// Earlier versions of this file used `dashboardModel` and every child page
// fell through to its default `null` binding, silently showing "…" forever.

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import com.visorcraft.LinSight

Kirigami.ApplicationWindow {
    id: root
    title: qsTr("LinSight — Window %1").arg(windowNumber)
    width: 1100
    height: 1080
    visible: true

    property QtObject dashModel: null
    property int windowNumber: 1

    pageStack.initialPage: Kirigami.Page {
        padding: 0
        ColumnLayout {
            anchors.fill: parent
            spacing: 0

            TabBar {
                id: bar
                Layout.fillWidth: true
                currentIndex: 0
                TabButton { text: qsTr("Overview") }
                TabButton { text: qsTr("GPUs") }
                TabButton { text: qsTr("Storage") }
                TabButton { text: qsTr("Network") }
            }

            StackLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                currentIndex: bar.currentIndex

                OverviewPage { dashModel: root.dashModel }
                CategoryPage { category: "gpu";     dashModel: root.dashModel; pageTitle: qsTr("GPUs"); groupBy: "deviceLabel" }
                CategoryPage { category: "storage"; dashModel: root.dashModel; pageTitle: qsTr("Storage") }
                CategoryPage { category: "network"; dashModel: root.dashModel; pageTitle: qsTr("Network") }
            }
        }
    }
}
