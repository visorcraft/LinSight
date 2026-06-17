// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Settings card — title + subtitle + slotted content. Used to
// visually group related toggles / read-only fields on the Settings
// page.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Rectangle {
    id: root
    Layout.fillWidth: true
    property string title: ""
    property string subtitle: ""
    default property alias contentChildren: contentColumn.children
    property alias content: contentSlot.sourceComponent

    color: app.tokens.surface1
    radius: app.tokens.radiusCard
    border.color: app.tokens.separator
    border.width: 1
    implicitHeight: column.implicitHeight + app.tokens.spaceL * 2

    ColumnLayout {
        id: column
        anchors.fill: parent
        anchors.margins: app.tokens.spaceL
        spacing: app.tokens.spaceM

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Controls.Label {
                text: root.title
                font.pixelSize: app.tokens.textSubheading
                font.weight: app.tokens.weightSemibold
                color: app.tokens.textPrimary
            }
            Controls.Label {
                text: root.subtitle
                visible: root.subtitle.length > 0
                opacity: 0.65
                font.pixelSize: app.tokens.textCaption + 1
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
                color: app.tokens.textPrimary
            }
        }

        Loader {
            id: contentSlot
            Layout.fillWidth: true
        }

        ColumnLayout {
            id: contentColumn
            Layout.fillWidth: true
            spacing: app.tokens.spaceS
        }
    }
}
