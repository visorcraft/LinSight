// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Per-sensor history dialog.
//
// Opened from SensorTile via `app.openHistory(sensorId, sensorName, unitLabel)`.
// Owns a single shared HistoryModel reference (set by Main.qml at app scope).
// Range pills drive historyModel.rangeMinutes + reload().

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Controls.Dialog {
    id: root

    // Set by Main.qml before open().
    property QtObject historyModel: null

    // Sensor metadata filled by openForSensor().
    property string sensorName: ""
    property string unitLabel: ""

    title: root.sensorName.length > 0 ? root.sensorName : qsTr("Sensor History")
    modal: true
    width: Math.min(app.width * 0.85, 760)
    height: Math.min(app.height * 0.80, 560)
    standardButtons: Controls.Dialog.Close

    // Parse pointsJson into a JS array; kept outside the Canvas so it is only
    // parsed once per update rather than once per paint call.
    property var parsedSamples: []

    function _parseSamples() {
        if (!root.historyModel) { root.parsedSamples = []; return }
        try {
            const raw = root.historyModel.pointsJson
            if (!raw || raw === "[]") { root.parsedSamples = []; return }
            root.parsedSamples = JSON.parse(raw)
        } catch (e) {
            root.parsedSamples = []
        }
    }

    Connections {
        target: root.historyModel
        function onPointsJsonChanged() { root._parseSamples() }
    }

    // Open for a specific sensor; loads the initial range.
    function openForSensor(sensorId, name, unit) {
        root.sensorName = name || sensorId
        root.unitLabel  = unit || ""
        root.parsedSamples = []
        root.selectedRange = 60
        if (root.historyModel) {
            root.historyModel.sensorId = sensorId
            root.historyModel.rangeMinutes = 60
            root.historyModel.reload()
        }
        root.open()
    }

    // Selected range in minutes — 15 / 60 / 1440 / 10080.
    property int selectedRange: 60

    // --- range pills ---------------------------------------------------------
    readonly property var rangePills: [
        { label: qsTr("15 m"),   minutes: 15    },
        { label: qsTr("1 h"),    minutes: 60    },
        { label: qsTr("24 h"),   minutes: 1440  },
        { label: qsTr("7 d"),    minutes: 10080 },
    ]

    // --- history-disabled detection ------------------------------------------
    // The daemon writes "history not enabled on daemon" verbatim when
    // LINSIGHT_HISTORY=1 is absent from its environment.
    readonly property bool isHistoryDisabled:
        root.historyModel !== null
        && root.historyModel.lastError.indexOf("history not enabled") !== -1

    contentItem: ColumnLayout {
        spacing: app.tokens.spaceM

        // Range pill row
        RowLayout {
            Layout.fillWidth: true
            spacing: app.tokens.spaceS

            Repeater {
                model: root.rangePills
                delegate: Controls.Button {
                    required property var modelData
                    text: modelData.label
                    flat: root.selectedRange !== modelData.minutes
                    highlighted: root.selectedRange === modelData.minutes
                    onClicked: {
                        root.selectedRange = modelData.minutes
                        if (root.historyModel) {
                            root.historyModel.rangeMinutes = modelData.minutes
                            root.historyModel.reload()
                        }
                    }
                }
            }

            Item { Layout.fillWidth: true }
        }

        // --- busy indicator ---------------------------------------------------
        Controls.BusyIndicator {
            Layout.alignment: Qt.AlignHCenter
            running: root.historyModel !== null && root.historyModel.isLoading
            visible: root.historyModel !== null && root.historyModel.isLoading
        }

        // --- history-disabled guidance ----------------------------------------
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Information
            visible: root.isHistoryDisabled && !root.historyModel.isLoading
            text: qsTr("History recording is not enabled on this daemon. "
                      + "Set LINSIGHT_HISTORY=1 in the daemon's environment "
                      + "and restart it to enable history.")
        }

        // --- generic error banner (non-disabled errors) -----------------------
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            visible: {
                if (!root.historyModel) return false
                const err = root.historyModel.lastError
                return err.length > 0 && !root.isHistoryDisabled && !root.historyModel.isLoading
            }
            text: root.historyModel ? root.historyModel.lastError : ""
        }

        // --- empty state ------------------------------------------------------
        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: 48
            visible: {
                if (!root.historyModel) return false
                if (root.historyModel.isLoading) return false
                if (root.historyModel.lastError.length > 0) return false
                return root.parsedSamples.length === 0
            }

            Controls.Label {
                anchors.centerIn: parent
                text: qsTr("No data in this range.")
                opacity: 0.55
                font.pixelSize: app.tokens.textBody
                color: app.tokens.textPrimary
            }
        }

        // --- chart ------------------------------------------------------------
        // Only shown when there's no error and we have at least one point.
        HistoryChart {
            Layout.fillWidth: true
            Layout.fillHeight: true
            visible: {
                if (!root.historyModel) return false
                if (root.historyModel.lastError.length > 0) return false
                return root.parsedSamples.length > 0
            }
            samples: root.parsedSamples
            accentColor: app.tokens.accent
            unitLabel: root.unitLabel
        }
    }
}
