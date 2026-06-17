// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Process explorer page — sortable, filterable table of running processes.
// Reads processesJson from the shared OverviewModel. Subscribes to
// proc.list on activation and unsubscribes on deactivation so the
// 5-second /proc sweep only runs while the page is visible.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import "qrc:/qml/Shared.js" as Shared

Kirigami.Page {
    id: page
    title: qsTr("Processes")
    padding: 0

    property QtObject dashModel: null

    Accessible.role: Accessible.Pane
    Accessible.name: qsTr("Processes")

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    Component.onCompleted: {
        if (page.dashModel) {
            page.dashModel.set_process_stream_enabled(true)
        }
    }

    Component.onDestruction: {
        if (page.dashModel) {
            page.dashModel.set_process_stream_enabled(false)
        }
    }

    readonly property var allProcesses: {
        if (!page.dashModel) return []
        try {
            const raw = JSON.parse(page.dashModel.processesJson || "[]")
            return Array.isArray(raw) ? raw : []
        } catch (e) {
            return []
        }
    }

    readonly property var filteredProcesses: {
        const term = filterField.text.toLowerCase()
        if (!term) return page.allProcesses
        return page.allProcesses.filter(p =>
            (p.name || "").toLowerCase().includes(term) ||
            String(p.pid !== undefined && p.pid !== null ? p.pid : "").includes(term)
        )
    }

    readonly property var sortedProcesses: {
        const col = page.sortColumn
        const dir = page.sortDirection
        const arr = page.filteredProcesses.slice()
        if (!col) return arr
        arr.sort((a, b) => {
            const av = a[col] !== undefined ? a[col] : 0
            const bv = b[col] !== undefined ? b[col] : 0
            if (typeof av === "string" && typeof bv === "string") {
                const cmp = av.localeCompare(bv)
                return dir === "asc" ? cmp : -cmp
            }
            const an = parseFloat(av) || 0
            const bn = parseFloat(bv) || 0
            return dir === "asc" ? an - bn : bn - an
        })
        return arr
    }

    property string sortColumn: app.preferences ? app.preferences.process_table_sort_column : "cpu"
    property string sortDirection: app.preferences && app.preferences.process_table_sort_descending ? "desc" : "asc"

    function toggleSort(col) {
        if (page.sortColumn === col) {
            page.sortDirection = page.sortDirection === "asc" ? "desc" : "asc"
        } else {
            page.sortColumn = col
            page.sortDirection = "desc"
        }
        if (app.preferences) {
            app.preferences.apply_process_table_sort(page.sortColumn, page.sortDirection === "desc")
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Header with filter
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
                    text: qsTr("Processes")
                    font.pixelSize: app.tokens.textHeading
                    font.weight: app.tokens.weightBold
                    font.family: app.tokens.sansFamily
                    color: app.tokens.textPrimary
                }
                Item { Layout.fillWidth: true }
                Controls.TextField {
                    id: filterField
                    Layout.preferredWidth: 240
                    Layout.alignment: Qt.AlignVCenter
                    placeholderText: qsTr("Filter by name or PID…")
                    selectByMouse: true
                    Accessible.name: qsTr("Filter processes")
                    rightPadding: clearFilterButton.width + app.tokens.spaceS
                    text: app.preferences ? app.preferences.process_table_filter_text : ""

                    onTextChanged: filterDebounceTimer.restart()

                    Timer {
                        id: filterDebounceTimer
                        interval: 300
                        onTriggered: {
                            if (app.preferences) {
                                app.preferences.apply_process_table_filter(filterField.text)
                            }
                        }
                    }

                    // Clear-filter affordance so a persisted hidden filter doesn't look broken.
                    Controls.ToolButton {
                        id: clearFilterButton
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        visible: filterField.text.length > 0
                        icon.name: "edit-clear"
                        flat: true
                        onClicked: filterField.text = ""
                        Controls.ToolTip.text: qsTr("Clear filter")
                        Controls.ToolTip.visible: hovered
                        Controls.ToolTip.delay: 400
                    }
                }
            }
        }

        Controls.Label {
            visible: filterField.text.length > 0
            text: qsTr("Filtering by: %1").arg(filterField.text)
            font.pixelSize: app.tokens.textCaption
            opacity: 0.7
            color: app.tokens.textPrimary
            Layout.leftMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceS
        }

        // Column headers
        Rectangle {
            Layout.fillWidth: true
            height: 36
            color: app.tokens.surface1
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceXL
                anchors.rightMargin: app.tokens.spaceXL
                spacing: 0

                HeaderCell { text: qsTr("PID"); sortKey: "pid"; widthFrac: 0.08 }
                HeaderCell { text: qsTr("Name"); sortKey: "name"; widthFrac: 0.25 }
                HeaderCell { text: qsTr("CPU %"); sortKey: "cpu"; widthFrac: 0.10 }
                HeaderCell { text: qsTr("Mem %"); sortKey: "mem"; widthFrac: 0.10 }
                HeaderCell { text: qsTr("RSS"); sortKey: "rss"; widthFrac: 0.14 }
                HeaderCell { text: qsTr("Threads"); sortKey: "threads"; widthFrac: 0.10 }
                HeaderCell { text: qsTr("State"); sortKey: "state"; widthFrac: 0.08 }
            }
        }

        // Process rows
        Controls.ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff

            ListView {
                id: listView
                clip: true
                model: page.sortedProcesses
                spacing: 0

                delegate: Rectangle {
                    required property var modelData
                    width: ListView.view.width
                    height: 32
                    color: index % 2 === 0 ? app.tokens.surface0 : app.tokens.surface1

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: app.tokens.spaceXL
                        anchors.rightMargin: app.tokens.spaceXL
                        spacing: 0

                        DataCell { text: modelData.pid !== undefined && modelData.pid !== null ? String(modelData.pid) : ""; widthFrac: 0.08 }
                        DataCell { text: modelData.name || ""; widthFrac: 0.25 }
                        DataCell { text: formatFloat(modelData.cpu) + "%"; widthFrac: 0.10 }
                        DataCell { text: formatFloat(modelData.mem) + "%"; widthFrac: 0.10 }
                        DataCell { text: Shared.formatBytes(modelData.rss); widthFrac: 0.14 }
                        DataCell { text: modelData.threads !== undefined && modelData.threads !== null ? String(modelData.threads) : ""; widthFrac: 0.10 }
                        DataCell { text: modelData.state || ""; widthFrac: 0.08 }
                    }
                }

                // Empty state
                Rectangle {
                    anchors.fill: parent
                    visible: listView.count === 0
                    color: app.tokens.surface0
                    ColumnLayout {
                        anchors.centerIn: parent
                        spacing: app.tokens.spaceM
                        Controls.Label {
                            text: filterField.text.length > 0 ? qsTr("No matching processes") : qsTr("No processes")
                            font.pixelSize: app.tokens.textSubheading
                            color: app.tokens.textSecondary
                            Layout.alignment: Qt.AlignHCenter
                        }
                        Controls.Label {
                            text: qsTr("Waiting for proc.list samples…")
                            font.pixelSize: app.tokens.textBody
                            color: app.tokens.textSecondary
                            Layout.alignment: Qt.AlignHCenter
                            visible: filterField.text.length === 0 && page.allProcesses.length === 0
                        }
                    }
                }
            }
        }
    }

    component HeaderCell: Controls.Button {
        property string sortKey: ""
        property real widthFrac: 0.1

        Layout.preferredWidth: parent.width * widthFrac
        Layout.fillHeight: true
        flat: true
        contentItem: RowLayout {
            spacing: 4
            Controls.Label {
                text: parent.parent.text
                font.pixelSize: app.tokens.textCaption
                font.weight: app.tokens.weightSemibold
                font.family: app.tokens.sansFamily
                color: app.tokens.textPrimary
                elide: Text.ElideRight
            }
            Controls.Label {
                visible: page.sortColumn === parent.parent.sortKey
                text: page.sortDirection === "asc" ? "▲" : "▼"
                font.pixelSize: app.tokens.textCaption - 2
                color: app.tokens.accent
            }
        }
        onClicked: page.toggleSort(sortKey)
    }

    component DataCell: Controls.Label {
        property real widthFrac: 0.1

        Layout.preferredWidth: parent.width * widthFrac
        Layout.fillHeight: true
        verticalAlignment: Text.AlignVCenter
        font.pixelSize: app.tokens.textBody
        font.family: app.tokens.monoFamily
        color: app.tokens.textPrimary
        elide: Text.ElideRight
    }

    function formatFloat(v) {
        const n = parseFloat(v)
        if (isNaN(n)) return "0.0"
        return n.toFixed(1)
    }

}
