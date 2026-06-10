// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    title: qsTr("Overview")
    padding: 0
    Accessible.role: Accessible.Pane
    Accessible.name: qsTr("Overview")

    // Receives the app-scope OverviewModel from Main.qml.
    property QtObject dashModel: null

    // Sparkline data extracted from tilesJson.
    property var _sparklines: ({})

    // Parse sparkline data whenever tilesJson updates
    Connections {
        target: page.dashModel
        function onTilesJsonChanged() {
            try {
                const arr = JSON.parse(page.dashModel.tilesJson || "[]")
                const sl = {}
                for (let i = 0; i < arr.length; ++i) {
                    const t = arr[i]
                    if (Array.isArray(t.sparkline) && t.sparkline.length >= 2) {
                        sl[t.id] = t.sparkline
                    }
                }
                page._sparklines = sl
            } catch (e) { /* keep previous */ }
        }
    }

    // A full-bleed surface rectangle behind everything else — the
    // Kirigami.Page `background` slot is repainted by Kirigami's own
    // QML control template (using Kirigami.Theme.View backgroundColor),
    // so an explicit Rectangle anchored to the page is the only path
    // that lets the active LinSight theme override the surface here.
    Rectangle {
        anchors.fill: parent
        color: app.tokens.surface0
    }

    Rectangle {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: app.tokens.pageHeaderHeight
        color: app.tokens.surface0
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 1
            color: app.tokens.separator
        }
        ColumnLayout {
            anchors.fill: parent
            anchors.leftMargin: app.tokens.spaceXL
            anchors.rightMargin: app.tokens.spaceXL
            spacing: 1
            Layout.alignment: Qt.AlignVCenter
            Controls.Label {
                text: qsTr("Overview")
                font.pixelSize: app.tokens.textHeading
                font.weight: app.tokens.weightBold
                font.family: app.tokens.sansFamily
            }
            Controls.Label {
                text: qsTr("Live CPU and memory at a glance.")
                opacity: 0.6
                font.pixelSize: app.tokens.textCaption + 1
            }
        }
    }

    GridLayout {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: app.tokens.spaceXL
        columns: 2
        rowSpacing: app.tokens.spaceL
        columnSpacing: app.tokens.spaceL

        // Top row: utilization (the two original tiles).
        SensorTile {
            Layout.fillWidth: true
            Layout.fillHeight: true
            tileName: qsTr("CPU")
            tileValue: page.dashModel ? page.dashModel.cpuText : "…"
            tileSparkline: page._sparklines["cpu.util"] || []
            tileSensorId: "cpu.util"
            tileUnit: "%"
            sparklinesEnabled: app.preferences ? app.preferences.sparklines : true
        }
        SensorTile {
            Layout.fillWidth: true
            Layout.fillHeight: true
            tileName: qsTr("Memory")
            tileValue: page.dashModel ? page.dashModel.memText : "…"
            tileSparkline: page._sparklines["mem.used_bytes"] || []
            tileSensorId: "mem.used_bytes"
            tileUnit: "B"
            sparklinesEnabled: app.preferences ? app.preferences.sparklines : true
        }

        SensorTile {
            Layout.fillWidth: true
            Layout.fillHeight: true
            tileName: qsTr("CPU temperature")
            tileValue: page.dashModel ? page.dashModel.cpuTempText : "…"
            tileSparkline: page._sparklines["cpu.temp_c"] || []
            tileSensorId: "cpu.temp_c"
            tileUnit: "°C"
            sparklinesEnabled: app.preferences ? app.preferences.sparklines : true
        }
        SensorTile {
            Layout.fillWidth: true
            Layout.fillHeight: true
            tileName: qsTr("CPU frequency")
            tileValue: page.dashModel ? page.dashModel.cpuFreqText : "…"
            tileSparkline: page._sparklines["cpu.freq_hz"] || []
            tileSensorId: "cpu.freq_hz"
            tileUnit: "Hz"
            sparklinesEnabled: app.preferences ? app.preferences.sparklines : true
        }
    }

}
