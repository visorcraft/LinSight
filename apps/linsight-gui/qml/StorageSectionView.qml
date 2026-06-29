// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

import "Shared.js" as Shared

// Scrollable column of physical-disk cards. Each disk card shows the disk's
// own sensors, then inset cards for the mounts that live on it. Orphan
// sections render as a plain card (no disk chrome, no mounts).
    Controls.ScrollView {
        id: view
        property var sections: []
        // Expansion state lives here (not on the delegates) so it survives the
        // ~1s model rebuild. Keyed by mount label (unique per mountpoint).
        property var expandedMounts: ({})
        // Per-device throughput lookup parsed from dashModel.diskJson.
        property var _diskRates: ({})
        function _refreshDiskRates() {
            if (!app.dashModel) { view._diskRates = {}; return }
            try {
                const arr = JSON.parse(app.dashModel.diskJson || "[]")
                const m = {}
                for (var i = 0; i < arr.length; i++) {
                    var e = arr[i]
                    if (e.device) m[e.device] = e
                }
                view._diskRates = m
            } catch (e) { view._diskRates = {} }
        }
        onSectionsChanged: view._refreshDiskRates()
        Connections {
            target: app.dashModel
            function onDiskJsonChanged() { view._refreshDiskRates() }
        }
        clip: true
    contentWidth: availableWidth

    ColumnLayout {
        width: view.availableWidth
        spacing: app.tokens.spaceL

        Repeater {
            model: view.sections
            delegate: Rectangle {
                required property var modelData
                Layout.fillWidth: true
                Layout.preferredHeight: card.implicitHeight + app.tokens.spaceL * 2
                radius: app.tokens.radiusCard
                color: app.tokens.surface1

                ColumnLayout {
                    id: card
                    anchors.fill: parent
                    anchors.margins: app.tokens.spaceL
                    spacing: app.tokens.spaceM

                    RowLayout {
                        Layout.fillWidth: true
                        Controls.Label {
                            text: modelData.label
                            font.pixelSize: app.tokens.textBody
                            font.weight: app.tokens.weightBold
                            font.family: app.tokens.sansFamily
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Label {
                            visible: modelData.kind === "disk"
                            text: {
                                var cap = ""
                                for (var i = 0; i < modelData.ownTiles.length; i++) {
                                    var t = modelData.ownTiles[i]
                                    if (t.id && String(t.id).endsWith("capacity_bytes")) { cap = t.value; break }
                                }
                                return cap
                            }
                            opacity: 0.6
                            font.pixelSize: app.tokens.textCaption
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        visible: modelData.kind === "disk"
                        spacing: app.tokens.spaceS
                        Controls.Label {
                            visible: {
                                var r = view._diskRates[modelData.device]
                                return r !== undefined && (r.read_bytes_per_sec > 0 || r.written_bytes_per_sec > 0)
                            }
                            text: {
                                var r = view._diskRates[modelData.device]
                                if (!r) return ""
                                var parts = []
                                if (r.read_bytes_per_sec > 0)
                                    parts.append("↓ " + Shared.formatByteRate(r.read_bytes_per_sec))
                                if (r.written_bytes_per_sec > 0)
                                    parts.append("↑ " + Shared.formatByteRate(r.written_bytes_per_sec))
                                return parts.join("  ")
                            }
                            font.pixelSize: app.tokens.textCaption
                            font.family: app.tokens.monoFamily
                            color: app.tokens.accent
                        }
                    }

                    GridLayout {
                        Layout.fillWidth: true
                        columns: Math.max(1, Math.floor(view.availableWidth / 240))
                        rowSpacing: app.tokens.spaceM
                        columnSpacing: app.tokens.spaceM
                        Repeater {
                            model: modelData.ownTiles
                            delegate: SensorTile {
                                required property var modelData
                                Layout.fillWidth: true
                                Layout.preferredHeight: 156
                                tileName: modelData.name
                                tileDeviceLabel: ""
                                tileValue: modelData.value
                                tileKind: modelData.kind || "scalar"
                                tileRows: modelData.rows || []
                                tileSensorId: (modelData.kind !== "table" && modelData.kind !== "state") ? (modelData.id || "") : ""
                                tileUnit: modelData.unit || ""
                                tileSparkline: modelData.sparkline || []
                                sparklinesEnabled: app.preferences ? app.preferences.sparklines : true
                            }
                        }
                    }

                    Repeater {
                        model: modelData.mounts
                        delegate: Rectangle {
                            id: mountCardRoot
                            required property var modelData
                            property bool expanded: view.expandedMounts[modelData.label] === true
                            Layout.fillWidth: true
                            Layout.leftMargin: app.tokens.spaceL
                            Layout.preferredHeight: mountCard.implicitHeight + app.tokens.spaceM * 2
                            radius: app.tokens.radiusCard
                            color: app.tokens.surface0
                            border.color: app.tokens.separator
                            border.width: 1

                            ColumnLayout {
                                id: mountCard
                                anchors.fill: parent
                                anchors.margins: app.tokens.spaceM
                                spacing: app.tokens.spaceM

                                // Clickable header: chevron + mount label. Toggles `expanded`.
                                Item {
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: mountHeader.implicitHeight
                                    RowLayout {
                                        id: mountHeader
                                        anchors.fill: parent
                                        spacing: app.tokens.spaceM
                                        Controls.Label {
                                            text: mountCardRoot.expanded ? "▾" : "▸"
                                            font.pixelSize: app.tokens.textCaption
                                            opacity: 0.7
                                        }
                                        Controls.Label {
                                            text: mountCardRoot.modelData.label
                                            font.pixelSize: app.tokens.textCaption
                                            font.weight: app.tokens.weightBold
                                            opacity: 0.7
                                        }
                                        Item { Layout.fillWidth: true }
                                    }
                                    MouseArea {
                                        anchors.fill: parent
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: view.expandedMounts = Object.assign({}, view.expandedMounts, { [mountCardRoot.modelData.label]: !mountCardRoot.expanded })
                                    }
                                }

                                GridLayout {
                                    Layout.fillWidth: true
                                    visible: mountCardRoot.expanded
                                    columns: Math.max(1, Math.floor((view.availableWidth - app.tokens.spaceL) / 240))
                                    rowSpacing: app.tokens.spaceM
                                    columnSpacing: app.tokens.spaceM
                                    Repeater {
                                        model: mountCardRoot.modelData.tiles
                                        delegate: SensorTile {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            Layout.preferredHeight: 156
                                            tileName: modelData.name
                                            tileDeviceLabel: ""
                                            tileValue: modelData.value
                                            tileKind: modelData.kind || "scalar"
                                            tileRows: modelData.rows || []
                                            tileSensorId: (modelData.kind !== "table" && modelData.kind !== "state") ? (modelData.id || "") : ""
                                            tileUnit: modelData.unit || ""
                                            tileSparkline: modelData.sparkline || []
                                            sparklinesEnabled: app.preferences ? app.preferences.sparklines : true
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
