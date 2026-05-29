// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Hardware page — lists every device the daemon detected, grouped
// as cards with inline nickname editing. Reads from `app.hardware`
// (HardwareModel), which fronts the daemon's `get_hardware` /
// `set_nickname` RPCs. Nickname commits also refresh the
// OverviewModel's tile labels via the daemon's
// `SensorListBroadcast` — that path is wired in Phase H and runs
// independently of this page.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    title: qsTr("Hardware")
    padding: 0

    Accessible.role: Accessible.Pane
    Accessible.name: qsTr("Hardware")

    Component.onCompleted: if (app.hardware) app.hardware.reload()

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    readonly property var devices: {
        if (!app.hardware) return []
        try {
            const raw = JSON.parse(app.hardware.devicesJson || "[]")
            return Array.isArray(raw) ? raw : []
        } catch (e) {
            return []
        }
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
                text: qsTr("Hardware")
                font.pixelSize: app.tokens.textHeading
                font.weight: app.tokens.weightBold
                font.family: app.tokens.sansFamily
            }
            Controls.Label {
                // Two separate qsTr strings, not `%1 device%2` with a
                // suffix arg — that pattern doesn't translate well into
                // languages with non-trivial plural rules.
                text: page.devices.length === 1
                    ? qsTr("%1 device").arg(1)
                    : qsTr("%1 devices").arg(page.devices.length)
                opacity: 0.6
                font.pixelSize: app.tokens.textCaption + 1
            }
        }
    }

    Controls.ScrollView {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: app.tokens.spaceXL
        anchors.rightMargin: app.tokens.spaceXL
        anchors.topMargin: app.tokens.spaceL
        anchors.bottomMargin: app.tokens.spaceL
        clip: true
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: app.tokens.spaceM

            Controls.Label {
                visible: page.devices.length === 0
                text: app.hardware && app.hardware.isLoading
                    ? qsTr("Detecting hardware…")
                    : qsTr("No hardware detected")
                opacity: 0.55
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: app.tokens.spaceXL
            }

            Controls.Label {
                visible: app.hardware
                    && app.hardware.lastError
                    && app.hardware.lastError.length > 0
                text: app.hardware ? app.hardware.lastError : ""
                color: "#ff8080"
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
            }

            Repeater {
                model: page.devices
                delegate: DeviceCard {
                    Layout.fillWidth: true
                    deviceJson: modelData
                }
            }
        }
    }

    component DeviceCard: Rectangle {
        id: card
        property var deviceJson
        // Coerce every field to a string defensively — the JSON came
        // through `JSON.parse`, but the daemon could surface a future
        // schema variant where a field is null or absent.
        property string deviceKey: deviceJson && deviceJson.key ? String(deviceJson.key) : ""
        // `label` is the daemon's authoritative display name —
        // nickname if set, otherwise the model possibly suffixed with
        // a location-derived disambiguator when two devices share a
        // model. Falls back to the raw model for older payloads that
        // didn't carry `label`.
        property string modelName: deviceJson && deviceJson.label
            ? String(deviceJson.label)
            : (deviceJson && deviceJson.model ? String(deviceJson.model) : qsTr("Unknown device"))
        property string vendorName: deviceJson && deviceJson.vendor ? String(deviceJson.vendor) : ""
        property string nickname: deviceJson && deviceJson.nickname ? String(deviceJson.nickname) : ""
        property int sensorCount: deviceJson && deviceJson.sensor_ids && deviceJson.sensor_ids.length !== undefined
            ? deviceJson.sensor_ids.length : 0

        // Tighter padding than the earlier draft so 8-9 cards fit at
        // the default 760px window height instead of 5.
        implicitHeight: cardContent.implicitHeight + 16
        color: app.tokens.surface1
        radius: 6

        ColumnLayout {
            id: cardContent
            anchors.fill: parent
            anchors.margins: 8
            spacing: 2

            // Single-row title + meta combo. Putting the device key
            // and sensor count on the same line as the model name
            // saves a whole row's worth of vertical space versus
            // stacking them.
            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceS
                Controls.Label {
                    text: card.modelName
                    font.pixelSize: app.tokens.textBody
                    font.weight: app.tokens.weightBold
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                }
                Controls.Label {
                    text: card.deviceKey + " · " + card.sensorCount + " " + qsTr("sensors")
                    opacity: 0.5
                    font.pixelSize: app.tokens.textCaption
                    elide: Text.ElideRight
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceS
                Controls.Label {
                    text: qsTr("Nickname:")
                    opacity: 0.7
                    font.pixelSize: app.tokens.textCaption
                }
                Controls.TextField {
                    id: nicknameField
                    Layout.fillWidth: true
                    placeholderText: qsTr("(none)")
                    text: card.nickname
                    maximumLength: 64
                    // Disable the field while a SetNickname RPC is in
                    // flight so the user can't fire a second edit on
                    // top of an unsettled one; a faint dim on the
                    // borderColor doubles as a "pending" cue.
                    enabled: !(app.hardware && app.hardware.isLoading)
                    opacity: enabled ? 1.0 : 0.6
                    onEditingFinished: {
                        if (card.deviceKey && app.hardware) {
                            app.hardware.applyNickname(card.deviceKey, text)
                        }
                    }
                }
            }
        }
    }
}
