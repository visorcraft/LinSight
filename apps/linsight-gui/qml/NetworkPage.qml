// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    title: qsTr("Network")
    padding: 0

    property QtObject dashModel: null

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    readonly property var interfaces: {
        if (!page.dashModel) return []
        try { return JSON.parse(page.dashModel.network_json || "[]") }
        catch (e) { return [] }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Rectangle {
            Layout.fillWidth: true
            height: app.tokens.pageHeaderHeight
            color: app.tokens.surface0
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: 1
                color: app.tokens.separator
            }
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceXL
                anchors.rightMargin: app.tokens.spaceXL
                spacing: app.tokens.spaceM
                Controls.Label {
                    text: qsTr("Network")
                    font.pixelSize: app.tokens.textHeading
                    font.weight: app.tokens.weightBold
                    font.family: app.tokens.sansFamily
                }
                Item { Layout.fillWidth: true }
                Controls.Label {
                    text: page.interfaces.length === 1
                        ? qsTr("%1 interface").arg(1)
                        : qsTr("%1 interfaces").arg(page.interfaces.length)
                    opacity: 0.6
                    font.pixelSize: app.tokens.textCaption + 1
                }
            }
        }

        Controls.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: availableWidth

            GridLayout {
                width: parent.width
                anchors.leftMargin: app.tokens.spaceXL
                anchors.rightMargin: app.tokens.spaceXL
                anchors.topMargin: app.tokens.spaceL
                anchors.bottomMargin: app.tokens.spaceL
                columns: Math.max(1, Math.floor(parent.width / 320))
                rowSpacing: app.tokens.spaceM
                columnSpacing: app.tokens.spaceM

                Repeater {
                    model: page.interfaces
                    delegate: NetworkCard {
                        Layout.fillWidth: true
                        iface: modelData
                    }
                }
            }
        }
    }

    Controls.Label {
        anchors.centerIn: parent
        text: qsTr("No network interfaces detected")
        visible: page.interfaces.length === 0
        opacity: 0.55
        font.pixelSize: app.tokens.textBody
    }
}
