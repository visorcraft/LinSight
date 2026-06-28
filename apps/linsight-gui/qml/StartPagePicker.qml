// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Two-stage Start Page selector. Primary dropdown picks a workspace
// or "Dashboard"; when "Dashboard" is chosen, a secondary dropdown
// appears with the user's saved dashboards. Persists via
// `PreferencesModel.applyStartPage`. The deleted-dashboard recovery
// path lives in DashboardsModel callers and Main.qml's boot logic —
// this page just edits the preference.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

ColumnLayout {
    id: root
    Layout.fillWidth: true
    spacing: app.tokens.spaceS

    // Primary slot — workspace key OR the literal "dashboard"
    // sentinel signalling "pick a slug from the secondary
    // dropdown".
    readonly property var primaryOptions: [
        { key: "overview", label: qsTr("Overview") },
        { key: "gpus",     label: qsTr("GPUs") },
        { key: "storage",  label: qsTr("Storage") },
        { key: "network",  label: qsTr("Network") },
        { key: "hardware", label: qsTr("Hardware") },
        { key: "dashboard", label: qsTr("Dashboard") },
    ]

    // Re-derive whenever the preferences model emits
    // startPageChanged. JSON-array models update by reference; this
    // tick ensures the binding refires on a notify-only change.
    readonly property string startPage: app.preferences
        ? app.preferences.startPage : "overview"

    // Re-derive whenever DashboardsModel.summaryJson changes
    // (created / renamed / removed) so the secondary list stays
    // current without a manual refresh.
    property var dashboards: []
    function refreshDashboards() {
        if (!app.dashboards) { dashboards = []; return }
        try { dashboards = JSON.parse(app.dashboards.summaryJson || "[]") }
        catch (e) { dashboards = [] }
    }
    Component.onCompleted: refreshDashboards()
    Connections {
        target: app.dashboards
        function onSummaryJsonChanged() { root.refreshDashboards() }
    }

    function isDashboardKey(s) { return String(s || "").indexOf("dashboard:") === 0 }
    function dashboardSlug(s)  { return isDashboardKey(s) ? String(s).substring("dashboard:".length) : "" }

    function primaryIndexFor(key) {
        if (root.isDashboardKey(key)) {
            // Last entry — "Dashboard".
            return root.primaryOptions.length - 1
        }
        for (let i = 0; i < root.primaryOptions.length; ++i) {
            if (root.primaryOptions[i].key === key) return i
        }
        return 0
    }

    function dashboardIndexFor(slug) {
        for (let i = 0; i < root.dashboards.length; ++i) {
            if (root.dashboards[i].slug === slug) return i
        }
        return -1
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: app.tokens.spaceM
        Controls.Label {
            text: qsTr("Open on launch")
            Layout.preferredWidth: 140
            opacity: 0.78
        }
        ThemedComboBox {
            id: primaryCombo
            Layout.fillWidth: true
            Layout.preferredHeight: 36
            model: root.primaryOptions
            textRole: "label"
            valueRole: "key"
            currentIndex: root.primaryIndexFor(root.startPage)
            onActivated: idx => {
                if (idx < 0 || idx >= root.primaryOptions.length) return
                const choice = root.primaryOptions[idx].key
                if (choice !== "dashboard") {
                    if (app.preferences) app.preferences.applyStartPage(choice)
                    return
                }
                // Pick the first dashboard slug as the default when
                // switching to "Dashboard". If there are no
                // dashboards yet, leave the preference as-is and let
                // the secondary dropdown surface the empty state.
                const slug = root.dashboards.length > 0
                    ? String(root.dashboards[0].slug)
                    : ""
                if (slug.length > 0 && app.preferences) {
                    app.preferences.applyStartPage("dashboard:" + slug)
                }
            }
        }
    }

    // Secondary dropdown — only shown when the primary is
    // "Dashboard". A user with no dashboards yet sees the empty
    // state with a hint to create one.
    RowLayout {
        Layout.fillWidth: true
        spacing: app.tokens.spaceM
        visible: primaryCombo.currentIndex === root.primaryOptions.length - 1
        Controls.Label {
            text: qsTr("Dashboard")
            Layout.preferredWidth: 140
            opacity: 0.78
        }
        ThemedComboBox {
            id: dashboardCombo
            Layout.fillWidth: true
            Layout.preferredHeight: 36
            model: root.dashboards
            textRole: "name"
            valueRole: "slug"
            enabled: root.dashboards.length > 0
            currentIndex: root.dashboardIndexFor(root.dashboardSlug(root.startPage))
            onActivated: idx => {
                if (idx < 0 || idx >= root.dashboards.length) return
                if (app.preferences) {
                    app.preferences.applyStartPage("dashboard:" + root.dashboards[idx].slug)
                }
            }
        }
    }

    Controls.Label {
        visible: primaryCombo.currentIndex === root.primaryOptions.length - 1
                 && root.dashboards.length === 0
        Layout.fillWidth: true
        text: qsTr("No dashboards yet. Use the sidebar's New Dashboard entry to create one.")
        opacity: 0.6
        font.pixelSize: app.tokens.textCaption
        wrapMode: Text.WordWrap
    }
}
