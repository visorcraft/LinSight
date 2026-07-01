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

            // Loader instantiates only the visible tab page. A StackLayout
            // would keep every CategoryPage live simultaneously, and since
            // each page's `tilesArray` binding re-derives every ~150 ms
            // (filter + group + sort), N tabs multiplied processing load
            // even when only one tab was visible. The Loader destroys the
            // previous page on tab switch so per-tick cost is bounded by
            // the single visible page.
            Loader {
                Layout.fillWidth: true
                Layout.fillHeight: true
                sourceComponent: {
                    switch (bar.currentIndex) {
                        case 0: return overviewComp
                        case 1: return gpuComp
                        case 2: return storageComp
                        case 3: return networkComp
                        default: return overviewComp
                    }
                }
            }
        }
    }

    Component {
        id: overviewComp
        OverviewPage { dashModel: root.dashModel }
    }
    Component {
        id: gpuComp
        CategoryPage {
            category: "gpu"
            dashModel: root.dashModel
            pageTitle: qsTr("GPUs")
            groupBy: "deviceLabel"
        }
    }
    Component {
        id: storageComp
        CategoryPage {
            category: "storage"
            dashModel: root.dashModel
            pageTitle: qsTr("Storage")
        }
    }
    Component {
        id: networkComp
        CategoryPage {
            category: "network"
            dashModel: root.dashModel
            pageTitle: qsTr("Network")
        }
    }
}
