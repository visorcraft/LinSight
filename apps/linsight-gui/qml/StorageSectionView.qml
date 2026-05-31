// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// Scrollable column of physical-disk cards. Each disk card shows the disk's
// own sensors, then inset cards for the mounts that live on it. Orphan
// sections render as a plain card (no disk chrome, no mounts).
Controls.ScrollView {
    id: view
    property var sections: []
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
                            }
                        }
                    }

                    Repeater {
                        model: modelData.mounts
                        delegate: Rectangle {
                            required property var modelData
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

                                Controls.Label {
                                    text: modelData.label
                                    font.pixelSize: app.tokens.textCaption
                                    font.weight: app.tokens.weightBold
                                    opacity: 0.7
                                }
                                GridLayout {
                                    Layout.fillWidth: true
                                    columns: Math.max(1, Math.floor((view.availableWidth - app.tokens.spaceL) / 240))
                                    rowSpacing: app.tokens.spaceM
                                    columnSpacing: app.tokens.spaceM
                                    Repeater {
                                        model: modelData.tiles
                                        delegate: SensorTile {
                                            required property var modelData
                                            Layout.fillWidth: true
                                            Layout.preferredHeight: 156
                                            tileName: modelData.name
                                            tileDeviceLabel: ""
                                            tileValue: modelData.value
                                            tileKind: modelData.kind || "scalar"
                                            tileRows: modelData.rows || []
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
