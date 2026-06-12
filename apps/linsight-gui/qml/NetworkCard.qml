// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Rectangle {
    id: root
    color: app.tokens.surface1
    radius: app.tokens.radiusCard
    implicitHeight: 180

    property var iface: null

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: app.tokens.spaceM
        spacing: app.tokens.spaceS

        RowLayout {
            Layout.fillWidth: true
            spacing: app.tokens.spaceS
            Controls.Label {
                text: root.iface ? root.iface.iface : ""
                font.pixelSize: app.tokens.textSubheading
                font.weight: app.tokens.weightBold
                color: app.tokens.textPrimary
            }
            Item { Layout.fillWidth: true }
            Rectangle {
                visible: root.iface && root.iface.link_state === "up"
                width: 8
                height: 8
                radius: 4
                color: app.tokens.positive
            }
            Controls.Label {
                text: root.iface ? root.iface.link_state : ""
                font.pixelSize: app.tokens.textCaption
                opacity: 0.7
                color: app.tokens.textPrimary
            }
        }

        Controls.Label {
            text: root.iface && root.iface.speed_mbps > 0
                ? qsTr("%1 Mbps").arg(root.iface.speed_mbps)
                : qsTr("Speed unknown")
            font.pixelSize: app.tokens.textCaption
            opacity: 0.6
            color: app.tokens.textPrimary
        }

        Item { Layout.fillHeight: true }

        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: app.tokens.spaceL
            rowSpacing: app.tokens.spaceS

            NetworkMetric { label: qsTr("RX"); value: root.iface ? root.iface.rx_bytes_per_sec : 0; isBytes: true }
            NetworkMetric { label: qsTr("TX"); value: root.iface ? root.iface.tx_bytes_per_sec : 0; isBytes: true }
            NetworkMetric { label: qsTr("RX pkt/s"); value: root.iface ? root.iface.rx_packets_per_sec : 0; isBytes: false }
            NetworkMetric { label: qsTr("TX pkt/s"); value: root.iface ? root.iface.tx_packets_per_sec : 0; isBytes: false }
            NetworkMetric { label: qsTr("Errors"); value: root.iface ? root.iface.rx_errors_per_sec + root.iface.tx_errors_per_sec : 0; isBytes: false }
            NetworkMetric { label: qsTr("Drops"); value: root.iface ? root.iface.rx_dropped_per_sec + root.iface.tx_dropped_per_sec : 0; isBytes: false }
        }
    }
}
