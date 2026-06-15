// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Settings page. v0.3 surfaces the env-var-controlled always-on
// subsystems (history, alerts, Prometheus) read-only, plus brief
// docs pointers. Mutable settings move here in a follow-up once
// linsightd grows a settings RPC endpoint.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.ScrollablePage {
    id: page
    padding: 0
    titleDelegate: Item {}
    globalToolBarStyle: Kirigami.ApplicationHeaderStyle.None

    // Shared OverviewModel — Main.qml passes it via `dashModel`. We
    // ask it `envIsSet(name)` to flip the indicator below from a fixed
    // checkbox-symbolic to a real on/off status.
    property QtObject dashModel: null

    // Env-var names that gate the always-on subsystems on the daemon
    // side. Keep these in sync with `apps/linsightd/src/runtime.rs`
    // where they are actually read. Treat any rename in Rust as a
    // breaking change here too.
    readonly property string envHistory: "LINSIGHT_HISTORY"
    readonly property string envAlerts:  "LINSIGHT_ALERTS"
    readonly property string envProm:    "LINSIGHT_PROM_BIND"

    property var daemonSettings: ({ history: false, alerts: false, prom: false, promBind: "" })

    function refreshDaemonSettings() {
        if (!page.dashModel) return
        const raw = page.dashModel.fetchDaemonSettings().toString()
        try {
            daemonSettings = JSON.parse(raw)
        } catch (e) {
            daemonSettings = { history: false, alerts: false, prom: false, promBind: "" }
        }
    }

    Component.onCompleted: page.refreshDaemonSettings()

    function escapeHtml(s) {
        return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }

    Kirigami.Theme.inherit: false
    Kirigami.Theme.colorSet: Kirigami.Theme.View
    Kirigami.Theme.backgroundColor: app.tokens.surface0
    Kirigami.Theme.textColor: app.tokens.textPrimary

    ColumnLayout {
        width: page.width
        spacing: 0

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: app.tokens.pageHeaderHeight
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
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1
                    Controls.Label {
                        text: qsTr("Settings")
                        font.pixelSize: app.tokens.textHeading
                        font.weight: app.tokens.weightBold
                        font.family: app.tokens.sansFamily
                    }
                    Controls.Label {
                        text: qsTr("Daemon behavior, sensors, and always-on subsystems.")
                        font.pixelSize: app.tokens.textCaption + 1
                        opacity: 0.6
                    }
                }
                // Re-read preferences.json from disk so a user who
                // edits the file by hand picks up the change without
                // restarting the GUI. Matches Grexa's header Reload
                // button; in LinSight today the only mutable
                // settings on disk are theme + active dashboard, but
                // future RPC-backed settings will plug in here.
                ThemedButton {
                    text: qsTr("Reload")
                    icon.name: "view-refresh"
                    Controls.ToolTip.text: qsTr("Re-read preferences.json from disk.")
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: {
                        if (app.preferences) app.preferences.reload()
                    }
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: app.tokens.spaceXL
            Layout.rightMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceL
            Layout.bottomMargin: app.tokens.spaceXL
            spacing: app.tokens.spaceL

            // Appearance is the most-touched setting — Grexa puts it
            // first in its Settings layout for the same reason.
            SettingsCard {
                title: qsTr("Appearance")
                subtitle: qsTr("Pick a built-in palette or follow your KDE Plasma color scheme.")
                content: ThemePicker {}
            }

            SettingsCard {
                title: qsTr("Start page")
                subtitle: qsTr("Which page LinSight opens on launch. A deleted dashboard falls back to Overview automatically.")
                content: StartPagePicker {}
            }

            SettingsCard {
                title: qsTr("Sample interval")
                subtitle: qsTr("How often the daemon checks for new sensor data. Lower values feel smoother; higher values reduce daemon CPU usage. The setting is per-client — changing it only affects this LinSight window.")
                content: RowLayout {
                    spacing: app.tokens.spaceM

                    ThemedComboBox {
                        id: sampleIntervalCombo
                        Layout.preferredWidth: 200
                        // Values in ms. 150 ms is the default — middle
                        // ground between 50 ms (smoothest, ~2.6% idle CPU)
                        // and 1000 ms (battery-saver, ~1% idle CPU).
                        textRole: "label"
                        valueRole: "ms"
                        model: ListModel {
                            ListElement { ms: 50;   label: "50 ms — Smoothest" }
                            ListElement { ms: 100;  label: "100 ms" }
                            ListElement { ms: 150;  label: "150 ms — Default" }
                            ListElement { ms: 200;  label: "200 ms" }
                            ListElement { ms: 250;  label: "250 ms" }
                            ListElement { ms: 350;  label: "350 ms" }
                            ListElement { ms: 500;  label: "500 ms" }
                            ListElement { ms: 750;  label: "750 ms" }
                            ListElement { ms: 1000; label: "1000 ms — Power saver" }
                        }
                        // Bind selected index to the preference value;
                        // fall back to the 150 ms slot if the persisted
                        // value isn't one of the offered choices.
                        Component.onCompleted: {
                            if (!app.preferences) return
                            const target = app.preferences.sampleIntervalMs
                            for (let i = 0; i < count; i++) {
                                if (model.get(i).ms === target) {
                                    currentIndex = i
                                    return
                                }
                            }
                            // Out-of-list value — snap to default.
                            for (let i = 0; i < count; i++) {
                                if (model.get(i).ms === 150) {
                                    currentIndex = i
                                    return
                                }
                            }
                        }
                        onActivated: {
                            if (app.preferences) {
                                app.preferences.applySampleIntervalMs(currentValue)
                            }
                        }
                    }

                    Controls.Label {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        opacity: 0.65
                        font.pixelSize: app.tokens.textCaption
                        text: qsTr("Default is 150 ms. The daemon clamps to %1–%2 ms.")
                            .arg(50).arg(1000)
                    }
                }
            }

            SettingsCard {
                title: qsTr("Tile sparklines")
                subtitle: qsTr("Show a mini trend chart inside each scalar sensor tile.")
                content: RowLayout {
                    spacing: app.tokens.spaceM
                    Controls.Switch {
                        id: sparklinesSwitch
                        checked: app.preferences ? app.preferences.sparklines : true
                        onToggled: {
                            if (app.preferences)
                                app.preferences.applySparklines(checked)
                        }
                        Connections {
                            target: app.preferences
                            function onSparklinesChanged() {
                                sparklinesSwitch.checked = app.preferences.sparklines
                            }
                        }
                    }
                    Controls.Label {
                        text: sparklinesSwitch.checked ? qsTr("Enabled") : qsTr("Disabled")
                        opacity: 0.7
                        font.pixelSize: app.tokens.textCaption + 1
                    }
                }
            }

            // Remote hosts section
            SettingsCard {
                title: qsTr("Remote hosts")
                subtitle: qsTr("Saved SSH targets for in-app switching. Key-based SSH authentication is assumed; the trust model is the same as launching with --connect ssh://....")
                content: ColumnLayout {
                    spacing: app.tokens.spaceM

                    Repeater {
                        model: {
                            try { return JSON.parse(app.hostsModel.hosts_json || "[]") }
                            catch (e) { return [] }
                        }
                        delegate: RowLayout {
                            Layout.fillWidth: true
                            spacing: app.tokens.spaceM
                            Controls.Label {
                                text: modelData.name
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                            }
                            Controls.Label {
                                text: modelData.url
                                opacity: 0.6
                                font.pixelSize: app.tokens.textCaption
                                Layout.maximumWidth: 220
                                elide: Text.ElideMiddle
                            }
                            ThemedButton {
                                text: qsTr("Remove")
                                icon.name: "list-remove-symbolic"
                                onClicked: app.hostsModel.remove(modelData.name)
                            }
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: app.tokens.spaceM
                        ThemedTextField {
                            id: newHostName
                            Layout.preferredWidth: 160
                            placeholderText: qsTr("Name")
                        }
                        ThemedTextField {
                            id: newHostUrl
                            Layout.fillWidth: true
                            placeholderText: qsTr("ssh://user@host:port")
                        }
                        ThemedButton {
                            text: qsTr("Add")
                            icon.name: "list-add-symbolic"
                            onClicked: {
                                app.hostsModel.add(newHostName.text, newHostUrl.text)
                                if (String(app.hostsModel.lastError || "").length === 0) {
                                    newHostName.text = ""
                                    newHostUrl.text = ""
                                }
                            }
                        }
                    }

                    Controls.Label {
                        readonly property string hostError: app.hostsModel
                            ? String(app.hostsModel.lastError || "") : ""
                        visible: hostError.length > 0
                        text: hostError
                        color: app.tokens.negative
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        font.pixelSize: app.tokens.textCaption
                    }
                }
            }

            // Always-on section
            SettingsCard {
                title: qsTr("Always-on mode")
                subtitle: qsTr("History, alerts, and Prometheus continue running after the GUI closes.")
                content: ColumnLayout {
                    spacing: app.tokens.spaceS
                    Controls.Label {
                        text: qsTr("Toggle subsystems at runtime. Changes take effect immediately on the daemon.")
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.85
                    }
                    Repeater {
                        model: [
                            { key: "history", name: qsTr("SQLite history") },
                            { key: "alerts",  name: qsTr("Alert rules") },
                            { key: "prom",    name: qsTr("Prometheus exporter") },
                        ]
                        delegate: RowLayout {
                            Layout.fillWidth: true
                            spacing: app.tokens.spaceM
                            readonly property bool isOn: {
                                var key = modelData.key
                                return page.daemonSettings[key] === true
                            }
                            readonly property bool canToggle: modelData.key !== "prom"
                            Controls.Switch {
                                checked: parent.isOn
                                enabled: parent.canToggle && page.dashModel !== null
                                onToggled: {
                                    if (!page.dashModel) return
                                    var result = page.dashModel.setDaemonSetting(
                                        modelData.key, checked
                                    ).toString()
                                    if (result.indexOf("error:") === 0) {
                                        app.showPassiveNotification(result, 4000)
                                    }
                                    page.refreshDaemonSettings()
                                }
                            }
                            Controls.Label {
                                text: modelData.name
                                Layout.fillWidth: true
                            }
                            Controls.Label {
                                text: parent.isOn ? qsTr("On") : qsTr("Off")
                                font.pixelSize: app.tokens.textCaption
                                opacity: 0.6
                            }
                        }
                    }

                    Controls.Label {
                        text: page.daemonSettings.prom
                            ? qsTr("Prometheus exporter is bound to %1. To change the bind address, set LINSIGHT_PROM_BIND and restart the daemon.").arg(page.escapeHtml(page.daemonSettings.promBind || ""))
                            : qsTr("Prometheus exporter is not configured. Set LINSIGHT_PROM_BIND and restart the daemon to enable it.")
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        opacity: 0.65
                        font.pixelSize: app.tokens.textCaption
                        visible: page.daemonSettings.prom !== undefined
                    }
                }
            }

            // CLI section
            SettingsCard {
                title: qsTr("Command-line tools")
                subtitle: qsTr("The CLI and daemon ship alongside the GUI.")
                content: ColumnLayout {
                    spacing: app.tokens.spaceS
                    Controls.Label {
                        text: qsTr("<code>linsight-cli list</code> shows every sensor the daemon advertises.<br>"
                                 + "<code>linsight-cli read &lt;sensor&gt; --count N</code> streams live values.<br>"
                                 + "<code>linsight-cli plugin new &lt;name&gt;</code> scaffolds a third-party plugin crate.")
                        textFormat: Text.RichText
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                }
            }
        }
    }
}
