// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Read-only renderer for a saved dashboard. Lays out tiles at the
// pixel coordinates persisted by the editor, with live values pulled
// from the shared OverviewModel (via valueById). Editing happens on
// CanvasEditorPage — this page is the "viewer".
//
// When `viewingSlug` is empty, shows a dashboard gallery / overview.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import QtQuick.Dialogs as Dialogs
import "Shared.js" as Shared

Kirigami.Page {
    id: page
    title: page.viewingSlug.length > 0 ? page.dashboardName : qsTr("Dashboards")
    padding: 0
    Accessible.role: Accessible.Pane
    Accessible.name: page.dashboardName

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    property string viewingSlug: ""
    property string dashboardName: ""
    property QtObject dashModel: null
    property var tiles: []
    property var valueById: ({})
    property var sensorMetaById: ({})
    property var rowsById: ({})
    property var kindById: ({})

    Component.onCompleted: {
        page.refreshSensors()
        page.reload()
    }

    Connections {
        target: page.dashModel
        function onTilesJsonChanged() { page.refreshSensors() }
        function onTilesChangedJsonChanged() { page.applyTileDelta() }
    }

    function applyTileDelta() {
        if (!page.dashModel) return
        try {
            const arr = JSON.parse(page.dashModel.tilesChangedJson || "[]")
            if (arr.length === 0) return
            const v = page.valueById
            const m = page.sensorMetaById
            const r = page.rowsById
            const k = page.kindById
            for (let i = 0; i < arr.length; ++i) {
                const t = arr[i]
                v[t.id] = t.value
                m[t.id] = {
                    name: t.name,
                    deviceLabel: t.deviceLabel || "",
                    category: t.category,
                    sparkline: t.sparkline || []
                }
                if (t.rows && t.rows.length > 0) r[t.id] = t.rows
                else delete r[t.id]
                if (t.kind) k[t.id] = t.kind
            }
            // Reassign NEW object references. QML `var` change detection is by
            // identity, so reassigning the same mutated-in-place object fires
            // no change signal and the `valueById[...]`/rows/kind bindings never
            // re-evaluate — dashboard tiles would freeze on their initial "…".
            // (Same trap as CategoryPage._mergeTiles.) Shallow copy is cheap.
            page.valueById = Object.assign({}, v)
            page.sensorMetaById = Object.assign({}, m)
            page.rowsById = Object.assign({}, r)
            page.kindById = Object.assign({}, k)
        } catch (e) { /* keep previous state */ }
    }

    Connections {
        target: app.dashboards
        function onSlugListJsonChanged() { page.buildGallery() }
        function onSummaryJsonChanged() { page.buildGallery() }
    }

    onViewingSlugChanged: page.reload()

    /// Evaluate a simple condition expression like "cpu.util > 50"
    /// against the current valueById map. Returns true if no condition
    /// is set or the condition evaluates truthy.
    function evalCondition(expr) {
        if (!expr || expr.trim().length === 0) return true
        // Match patterns: sensor_id OP number
        const m = expr.trim().match(/^([\w.]+)\s*(>=|<=|!=|==|>|<)\s*([\d.]+)$/)
        if (!m) return true  // unparseable → show
        const sid = m[1]
        const op = m[2]
        const val = parseFloat(m[3])
        const live = parseFloat(page.valueById[sid] || "NaN")
        if (isNaN(live)) return true
        switch (op) {
            case ">":  return live > val
            case "<":  return live < val
            case ">=": return live >= val
            case "<=": return live <= val
            case "==": return live === val
            case "!=": return live !== val
            default:   return true
        }
    }

    /// Copy the current dashboard layout to the clipboard as JSON.
    function copyDashboardToClipboard() {
        if (!page.viewingSlug || !app.dashboards) return
        const json = app.dashboards.loadLayout(page.viewingSlug).toString()
        const name = page.dashboardName || page.viewingSlug
        const blob = JSON.stringify({
            schema_version: 1,
            name: name,
            slug: page.viewingSlug,
            layout: JSON.parse(json),
            exported_at: new Date().toISOString()
        }, null, 2)
        Clipboard.text = blob
        app.showPassiveNotification(
            qsTr("Dashboard layout copied to clipboard — paste into a file to share."),
            5000
        )
    }

    function refreshSensors() {
        if (!page.dashModel) return
        try {
            const arr = JSON.parse(page.dashModel.tilesJson || "[]")
            const v = {}
            const m = {}
            const r = {}
            const k = {}
            for (let i = 0; i < arr.length; ++i) {
                const t = arr[i]
                v[t.id] = t.value
                m[t.id] = {
                    name: t.name,
                    deviceLabel: t.deviceLabel || "",
                    category: t.category,
                    sparkline: t.sparkline || []
                }
                if (t.rows && t.rows.length > 0) r[t.id] = t.rows
                if (t.kind) k[t.id] = t.kind
            }
            page.valueById = v
            page.sensorMetaById = m
            page.rowsById = r
            page.kindById = k
        } catch (e) { /* keep previous state */ }
    }

    function canvasContentWidth() {
        if (!page.tiles || !Array.isArray(page.tiles)) return width
        let max = 0
        for (let i = 0; i < page.tiles.length; ++i) {
            const t = page.tiles[i]
            max = Math.max(max, (Number(t.x) || 0) + (Number(t.w) || 200))
        }
        return max + app.tokens.spaceM
    }

    function canvasContentHeight() {
        if (!page.tiles || !Array.isArray(page.tiles)) return height
        let max = 0
        for (let i = 0; i < page.tiles.length; ++i) {
            const t = page.tiles[i]
            max = Math.max(max, (Number(t.y) || 0) + (Number(t.h) || 120))
        }
        return max + app.tokens.spaceM
    }

    function reload() {
        if (!page.viewingSlug || !app.dashboards) {
            page.tiles = []
            page.dashboardName = ""
            return
        }
        page.dashboardName = app.dashboards.nameOf(page.viewingSlug).toString()
        try {
            const arr = JSON.parse(app.dashboards.loadLayout(page.viewingSlug).toString())
            page.tiles = Array.isArray(arr) ? arr : []
        } catch (e) {
            page.tiles = []
        }
    }

    Dialogs.FileDialog {
        id: importDialog
        title: qsTr("Import Dashboard")
        nameFilters: ["Dashboard JSON (*.dashboard.json *.json)", "All files (*)"]
        onAccepted: {
            const result = app.dashboards.importDashboard(importDialog.selectedFile.toString())
            if (result.length > 0) {
                app.goTo("dashboard:" + result)
                app.showPassiveNotification(qsTr("Imported dashboard"), 3000)
            } else {
                app.showPassiveNotification(app.dashboards.lastError || qsTr("Import failed"), 4000)
            }
        }
    }

    // ── GALLERY VIEW (when no slug selected) ──────────────────────
    Rectangle {
        anchors.fill: parent
        visible: page.viewingSlug.length === 0
        color: app.tokens.surface0

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: app.tokens.spaceXL
            spacing: app.tokens.spaceM

            Controls.Label {
                text: qsTr("Your Dashboards")
                font.pixelSize: app.tokens.textHeading
                font.weight: app.tokens.weightBold
                color: app.tokens.textPrimary
                Layout.fillWidth: true
            }

            // Toolbar
            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceM
                Item { Layout.fillWidth: true }
                Controls.Button {
                    icon.name: "document-open-symbolic"
                    text: qsTr("Import")
                    onClicked: importDialog.open()
                }
                Controls.Button {
                    icon.name: "list-add-symbolic"
                    text: qsTr("New Dashboard")
                    onClicked: app.newDashboardDialog.open()
                }
            }

            // Dashboard cards
            Controls.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true

                Flow {
                    id: cardFlow
                    width: parent.width
                    spacing: app.tokens.spaceM

                    Component.onCompleted: {
                        page.buildGallery()
                    }
                }
            }
        }
    }

    function buildGallery() {
        if (!app.dashboards) return
        cardFlow.data = []

        // Enumerate dashboards from the model's slugListJson.
        const slugs = JSON.parse(app.dashboards.slugListJson || "[]")
        for (let i = 0; i < slugs.length; ++i) {
            const slug = slugs[i]
            const name = app.dashboards.nameOf(slug).toString()
            if (name.length === 0) continue
            addCard(slug, name)
        }
    }

    function addCard(slug, name) {
        const card = Qt.createQmlObject(`
            import QtQuick
            import QtQuick.Controls as Controls
            import QtQuick.Layouts
            import org.kde.kirigami as Kirigami
            Rectangle {
                width: 240
                height: 160
                radius: app.tokens.radiusCard
                color: app.tokens.surface1
                border.width: 1
                border.color: app.tokens.separator

                property string cardSlug: ""
                property string cardName: ""

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: app.goTo("dashboard:" + parent.cardSlug)
                }

                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: app.tokens.spaceM
                    spacing: app.tokens.spaceS
                    Controls.Label {
                        text: parent.parent.cardName
                        font.pixelSize: app.tokens.textBody
                        font.weight: app.tokens.weightBold
                        color: app.tokens.textPrimary
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        text: qsTr("Preset dashboard")
                        opacity: 0.6
                        font.pixelSize: app.tokens.textCaption
                        color: app.tokens.textPrimary
                        Layout.fillWidth: true
                    }
                    Item { Layout.fillHeight: true }
                    RowLayout {
                        Layout.fillWidth: true
                        spacing: app.tokens.spaceXS
                        Controls.Button {
                            icon.name: "document-edit-symbolic"
                            text: qsTr("Open")
                            flat: true
                            onClicked: app.goTo("dashboard:" + parent.parent.parent.cardSlug)
                        }
                        Item { Layout.fillWidth: true }
                        Controls.Button {
                            icon.name: "document-edit-symbolic"
                            text: qsTr("Edit")
                            flat: true
                            onClicked: app.goTo("editor:" + parent.parent.parent.cardSlug)
                        }
                    }
                }
            }
        `, cardFlow, "card_" + slug)
        card.cardSlug = slug
        card.cardName = name
    }

    // ── DASHBOARD VIEW (when slug is selected) ────────────────────
    Rectangle {
        anchors.fill: parent
        visible: page.viewingSlug.length > 0 && app.dashboards
        color: app.tokens.surface0

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
            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceXL
                anchors.rightMargin: app.tokens.spaceXL
                spacing: app.tokens.spaceM
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1
                    Controls.Label {
                        text: page.dashboardName.length > 0 ? page.dashboardName : qsTr("Dashboard")
                        font.pixelSize: app.tokens.textHeading
                        font.weight: app.tokens.weightBold
                        font.family: app.tokens.sansFamily
                        color: app.tokens.textPrimary
                    }
                    Controls.Label {
                        text: qsTr("%1 tile(s)").arg(page.tiles.length)
                        opacity: 0.6
                        font.pixelSize: app.tokens.textCaption + 1
                        color: app.tokens.textPrimary
                    }
                }
                ThemedButton {
                    icon.name: "edit-copy-symbolic"
                    text: qsTr("Copy to clipboard")
                    Controls.ToolTip.text: qsTr("Copy the dashboard JSON to the clipboard")
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    onClicked: page.copyDashboardToClipboard()
                }
                ThemedButton {
                    icon.name: "document-edit-symbolic"
                    text: qsTr("Edit")
                    onClicked: app.goTo("editor:" + page.viewingSlug)
                }
                ThemedButton {
                    icon.name: "go-home-symbolic"
                    text: qsTr("Gallery")
                    onClicked: {
                        page.viewingSlug = ""
                        // goTo() short-circuits when `key === currentPageKey`.
                        // Leaving it at the viewed slug would make the next
                        // "Open" of that same dashboard a no-op, and wrongly
                        // keep its nav row highlighted in the gallery.
                        app.currentPageKey = "dashboards"
                    }
                }
            }
        }

        Rectangle {
            id: canvas
            anchors.top: header.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            color: app.tokens.surface0

            Controls.Label {
                anchors.centerIn: parent
                visible: page.tiles.length === 0
                text: qsTr("This dashboard is empty. Open it in the Editor to add sensors.")
                opacity: 0.5
                color: app.tokens.textPrimary
                font.pixelSize: app.tokens.textBody
            }

            Controls.ScrollView {
                anchors.fill: parent
                clip: true
                contentWidth: Math.max(page.canvasContentWidth(), width)
                contentHeight: Math.max(page.canvasContentHeight(), height)

                Repeater {
                    model: page.tiles
                    delegate: Rectangle {
                        x: Number(modelData.x) || 0
                        y: Number(modelData.y) || 0
                        width: Number(modelData.w) || 200
                        height: Number(modelData.h) || 120
                        radius: app.tokens.radiusCard
                        color: app.tokens.surface1
                        border.width: {
                            var opts = modelData.options || {}
                            if (!opts.thresholdEnabled) return 1
                            var numVal = parseFloat(page.valueById[String(modelData.id || "")])
                            if (isNaN(numVal)) return 1
                            if (opts.thresholdWarn && numVal >= parseFloat(opts.thresholdWarn)) return 2
                            if (opts.thresholdOk && numVal >= parseFloat(opts.thresholdOk)) return 2
                            return 1
                        }
                        border.color: {
                            var opts = modelData.options || {}
                            if (!opts.thresholdEnabled) return app.tokens.separator
                            var numVal = parseFloat(page.valueById[String(modelData.id || "")])
                            if (isNaN(numVal)) return app.tokens.separator
                            if (opts.thresholdWarn && numVal >= parseFloat(opts.thresholdWarn)) return app.tokens.negative
                            if (opts.thresholdOk && numVal >= parseFloat(opts.thresholdOk)) return app.tokens.warning
                            return app.tokens.separator
                        }

                        readonly property string sid: String(modelData.id || "")
                        readonly property var meta: page.sensorMetaById[sid] || ({})
                        readonly property var tileOpts: modelData.options || ({})
                        readonly property bool visibleByCondition: {
                            var opts = modelData.options || {}
                            var cond = opts.condition || ""
                            return page.evalCondition(cond)
                        }

                        visible: visibleByCondition

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: app.tokens.spaceM
                            spacing: app.tokens.spaceXS
                            Controls.Label {
                                text: {
                                    var opts = parent.parent.tileOpts
                                    var base = opts.labelOverride || (parent.parent.meta.name || parent.parent.sid)
                                    return base + (parent.parent.visibleByCondition ? "" : qsTr(" (hidden)"))
                                }
                                font.pixelSize: app.tokens.textCaption + 1
                                font.weight: app.tokens.weightSemibold
                                opacity: 0.7
                                color: {
                                    var opts = parent.parent.tileOpts
                                    return opts.textAccent || app.tokens.textPrimary
                                }
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            // Second title line: the hardware device this
                            // metric belongs to (nickname || model). Present
                            // only for device-scoped sensors.
                            Controls.Label {
                                visible: (parent.parent.meta.deviceLabel || "").length > 0
                                text: parent.parent.meta.deviceLabel || ""
                                font.pixelSize: app.tokens.textCaption
                                opacity: 0.55
                                color: app.tokens.textPrimary
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Loader {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                sourceComponent: {
                                    var kind = page.kindById[parent.parent.sid] || "scalar"
                                    if (kind === "table" && page.rowsById[parent.parent.sid]) return dashTableComp
                                    return dashScalarComp
                                }
                                property string _sid: parent.parent.sid
                            }
                            // C1 — Mini sparkline chart (gated by preference)
                            HistoryChart {
                                Layout.fillWidth: true
                                height: 34
                                mini: true
                                accentColor: app.tokens.accent
                                values: {
                                    const meta = parent.parent && parent.parent.sid ? page.sensorMetaById[parent.parent.sid] : null
                                    return meta && Array.isArray(meta.sparkline) ? meta.sparkline : []
                                }
                                visible: {
                                    if (!(app.preferences ? app.preferences.sparklines : true)) return false
                                    const kind = page.kindById[parent.parent.sid] || "scalar"
                                    if (kind === "table" || kind === "state") return false
                                    return Shared.sparklineVaries(values)
                                }
                            }
                            Item { Layout.fillHeight: true }
                            Controls.Label {
                                text: parent.parent.meta.category || ""
                                font.pixelSize: app.tokens.textCaption
                                opacity: 0.45
                                color: app.tokens.textPrimary
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                        }
                    }
                }
            }
        }
    }

    Component {
        id: dashScalarComp
        Controls.Label {
            text: page.valueById[_sid] || "…"
            font.pixelSize: app.tokens.textDisplay
            font.weight: app.tokens.weightBold
            font.family: app.tokens.monoFamily
            color: app.tokens.textPrimary
            elide: Text.ElideRight
            Layout.fillWidth: true
        }
    }

    Component {
        id: dashTableComp
        Controls.ScrollView {
            clip: true
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
            ListView {
                model: page.rowsById[_sid] || []
                interactive: true
                boundsBehavior: Flickable.StopAtBounds
                delegate: RowLayout {
                    width: tableView.availableWidth
                    spacing: app.tokens.spaceXS
                    Repeater {
                        model: modelData
                        delegate: Controls.Label {
                            text: {
                                if (typeof modelData === 'object' && modelData !== null) {
                                    if (modelData.text !== undefined) return modelData.text;
                                    if (modelData.number !== undefined) return Number(modelData.number).toFixed(1);
                                    if (modelData.bytes !== undefined) return Shared.formatBytes(modelData.bytes);
                                    return "";
                                }
                                return String(modelData);
                            }
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                            color: app.tokens.textPrimary
                            font.pixelSize: app.tokens.textCaption
                        }
                    }
                }
            }
        }
    }

}