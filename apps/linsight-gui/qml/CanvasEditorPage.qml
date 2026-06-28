// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Phase 6b — Canvas editor.
//
// Three zones (left palette, center canvas, right save/load strip).
// Sensors are dragged from the palette onto the free-positioned canvas;
// dropped tiles can be moved and resized via a corner handle; all
// coordinates snap to an 8px grid. Save serializes a flat
// `[{id,x,y,w,h}]` array to the dashboard JSON via
// DashboardsModel::save_layout. Load round-trips it back.
//
// Live values come from the app-scope OverviewModel via the
// `dashModel` property. The Connections block fires refreshSensors()
// whenever new samples arrive.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import "Shared.js" as Shared

Kirigami.Page {
    id: page
    title: qsTr("Editor")
    padding: 0
    Accessible.role: Accessible.Pane
    Accessible.name: qsTr("Canvas editor")

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    readonly property int gridStep: 8

    // Stable category ordering for the palette sort comparator.
    // Built once so the comparator does not allocate an array per call.
    readonly property var categoryOrderMap: {
        const m = {}
        m[qsTr("CPU Sensors")] = 0
        m[qsTr("Memory Sensors")] = 1
        m[qsTr("GPU Sensors")] = 2
        m[qsTr("Storage Sensors")] = 3
        m[qsTr("Network Sensors")] = 4
        m[qsTr("Thermal Sensors")] = 5
        m[qsTr("Power Sensors")] = 6
        m[qsTr("Battery Sensors")] = 7
        m[qsTr("Other Sensors")] = 8
        return m
    }

    // Receives the shared OverviewModel from Main.qml.
    property QtObject dashModel: null

    // Slug of the dashboard being edited. Empty means "no dashboard
    // selected" — Main.qml resolves this before pushing the page, so
    // an empty slug here is a programming bug, not a runtime state.
    // Switching the slug auto-loads the layout from disk so the same
    // page instance can edit different dashboards across navigations.
    property string editingSlug: ""

    // `nameOf` is a `Q_INVOKABLE` rather than a `Q_PROPERTY`, so QML
    // does not track the read as a binding dependency. Without a manual
    // re-evaluation tick the header could go stale after a rename that
    // preserved the slug. `_nameTick` bumps on every `summaryJson`
    // notify (and on slug change), forcing the binding to re-call.
    property int _nameTick: 0
    readonly property string editingName: {
        const _ = page._nameTick      // pull binding dep
        if (page.editingSlug.length === 0 || !app.dashboards) return ""
        return app.dashboards.nameOf(page.editingSlug).toString()
    }

    Component.onCompleted: {
        page.refreshSensors()
        page.autoLoadFromSlug()
    }
    Connections {
        target: page.dashModel
        function onTilesJsonChanged() { page.refreshSensors() }
        function onTilesChangedJsonChanged() { page.applyTileDelta() }
    }
    Connections {
        target: app.dashboards
        function onSummaryJsonChanged() { page._nameTick++ }
    }
    onEditingSlugChanged: {
        page._nameTick++
        page.autoLoadFromSlug()
    }

    function autoLoadFromSlug() {
        if (!page.editingSlug || !app.dashboards) {
            canvasModel.clear()
            page._canvasTick++
            page.statusText = ""
            return
        }
        const raw = app.dashboards.loadLayout(page.editingSlug).toString()
        page.loadFromJson(raw)
        page.statusText = qsTr("Editing %1").arg(page.editingName)
    }

    // ----- Sensor catalogue --------------------------------------------------
    //
    // `sensors` is the palette source: every entry has id/name/category.
    // It is built ONCE per ID-set change, NOT rebuilt on every sample
    // tick — rebuilding the array reference reseats the ListView's
    // delegates and snaps scrollY back to 0, making the palette
    // unscrollable on a live system. `valueById` is updated every
    // tick; palette rows read it for the live value via a binding so
    // values refresh in place without invalidating the model.
    property var sensors: []
    property var valueById: ({})
    property var rowsById: ({})
    property var kindById: ({})
    property string statusText: ""

    // True while the user is actively dragging or resizing a tile.
    // `refreshSensors` short-circuits when this is set so the
    // sample-driven repaint storm (every ~500ms the daemon publishes
    // a fresh tilesJson, which would otherwise rebuild `valueById`
    // and reseat every tile's value-label binding) doesn't compete
    // with the drag for the render thread. The skipped tick just
    // means tiles render their previous value for one cycle —
    // invisible to the user, who is watching the cursor.
    property bool _isDragging: false

    function refreshSensors() {
        if (!page.dashModel) return
        // While a drag is in progress, hold the catalogue stable so
        // the sample-stream tick doesn't trigger a full
        // value-label re-bind on every tile mid-drag. The cost is
        // one stale frame; the win is a fluid cursor.
        if (page._isDragging) return
        try {
            const arr = JSON.parse(page.dashModel.tilesJson || "[]")
            // Always update the value map.
            const lookup = {}
            const rowsLookup = {}
            const kindLookup = {}
            const incomingIds = new Array(arr.length)
            for (let i = 0; i < arr.length; ++i) {
                const t = arr[i]
                lookup[t.id] = t.value
                if (t.rows && t.rows.length > 0) rowsLookup[t.id] = t.rows
                if (t.kind) kindLookup[t.id] = t.kind
                incomingIds[i] = t.id
            }
            page.valueById = lookup
            page.rowsById = rowsLookup
            page.kindById = kindLookup
            // Only rebuild the palette array when the ID set actually
            // changed (sensor added, removed, or order shifted). The
            // common case — same sensors, new values — leaves the
            // ListView model reference untouched so scroll position is
            // preserved.
            const currentIds = new Array(page.sensors.length)
            for (let j = 0; j < page.sensors.length; ++j) {
                currentIds[j] = page.sensors[j].id
            }
            if (!page.sameStringSequence(incomingIds, currentIds)) {
                const next = []
                for (let i = 0; i < arr.length; ++i) {
                    const t = arr[i]
                    next.push({
                        id: t.id,
                        name: t.name,
                        deviceLabel: t.deviceLabel || "",
                        category: page.normalizeCategory(t.category),
                    })
                }
                // Sort by (category rank, display name) so the palette
                // renders predictable groups in a stable order. The
                // ListView.section delegate below paints headers
                // between groups; without the sort, the section
                // delegate would emit a new header every time the
                // category changes mid-stream.
                next.sort((a, b) => {
                    const ra = page.categoryRank(a.category)
                    const rb = page.categoryRank(b.category)
                    if (ra !== rb) return ra - rb
                    const na = a.name.toLowerCase()
                    const nb = b.name.toLowerCase()
                    return na < nb ? -1 : (na > nb ? 1 : 0)
                })
                page.sensors = next
            }
            canvasModel.refreshValues()
        } catch (e) {
            // Malformed JSON early in startup — keep prior catalogue.
        }
    }

    function applyTileDelta() {
        if (!page.dashModel) return
        if (page._isDragging) return
        try {
            const arr = JSON.parse(page.dashModel.tilesChangedJson || "[]")
            if (arr.length === 0) return
            const lookup = page.valueById
            const rowsLookup = page.rowsById
            const kindLookup = page.kindById
            for (let i = 0; i < arr.length; ++i) {
                const t = arr[i]
                lookup[t.id] = t.value
                if (t.rows && t.rows.length > 0) rowsLookup[t.id] = t.rows
                else delete rowsLookup[t.id]
                if (t.kind) kindLookup[t.id] = t.kind
            }
            // New references so the change signals fire — QML `var` change
            // detection is by identity, so reassigning the same mutated-in-place
            // object is ignored and tile bindings freeze. (Same trap as
            // CategoryPage._mergeTiles.) Shallow copy is cheap.
            page.valueById = Object.assign({}, lookup)
            page.rowsById = Object.assign({}, rowsLookup)
            page.kindById = Object.assign({}, kindLookup)
            canvasModel.refreshValues()
        } catch (e) { /* keep previous state */ }
    }

    function sameStringSequence(a, b) {
        if (a.length !== b.length) return false
        for (let i = 0; i < a.length; ++i) {
            if (a[i] !== b[i]) return false
        }
        return true
    }

    // Normalise the daemon-emitted category string (lowercase) into a
    // user-visible group name. Unknown / missing categories collapse
    // to "Other Sensors" so a new sensor plugin doesn't break the
    // palette grouping.
    function normalizeCategory(cat) {
        const lc = String(cat || "").toLowerCase()
        switch (lc) {
            case "cpu":     return qsTr("CPU Sensors")
            case "memory":  return qsTr("Memory Sensors")
            case "gpu":     return qsTr("GPU Sensors")
            case "storage": return qsTr("Storage Sensors")
            case "network": return qsTr("Network Sensors")
            case "battery": return qsTr("Battery Sensors")
            case "thermal": return qsTr("Thermal Sensors")
            case "power":   return qsTr("Power Sensors")
            default:        return qsTr("Other Sensors")
        }
    }

    // Rank categories so the palette lists them in a stable,
    // user-friendly order independent of arrival order from the
    // daemon (which is plugin-load order).
    function categoryRank(cat) {
        const i = page.categoryOrderMap[cat]
        return i === undefined ? 9 : i
    }

    function isValidNumber(s) {
        if (!s || s.trim().length === 0) return true
        const n = Number(s)
        return !isNaN(n) && isFinite(n)
    }

    function isValidColorString(s) {
        if (!s || s.trim().length === 0) return true
        if (/^#[0-9A-Fa-f]{3}([0-9A-Fa-f]{3})?([0-9A-Fa-f]{2})?$/.test(s)) return true
        const named = {
            aliceblue:1, antiquewhite:1, aqua:1, aquamarine:1, azure:1, beige:1, bisque:1,
            black:1, blanchedalmond:1, blue:1, blueviolet:1, brown:1, burlywood:1, cadetblue:1,
            chartreuse:1, chocolate:1, coral:1, cornflowerblue:1, cornsilk:1, crimson:1, cyan:1,
            darkblue:1, darkcyan:1, darkgoldenrod:1, darkgray:1, darkgreen:1, darkgrey:1,
            darkkhaki:1, darkmagenta:1, darkolivegreen:1, darkorange:1, darkorchid:1, darkred:1,
            darksalmon:1, darkseagreen:1, darkslateblue:1, darkslategray:1, darkslategrey:1,
            darkturquoise:1, darkviolet:1, deeppink:1, deepskyblue:1, dimgray:1, dimgrey:1,
            dodgerblue:1, firebrick:1, floralwhite:1, forestgreen:1, fuchsia:1, gainsboro:1,
            ghostwhite:1, gold:1, goldenrod:1, gray:1, green:1, greenyellow:1, grey:1,
            honeydew:1, hotpink:1, indianred:1, indigo:1, ivory:1, khaki:1, lavender:1,
            lavenderblush:1, lawngreen:1, lemonchiffon:1, lightblue:1, lightcoral:1, lightcyan:1,
            lightgoldenrodyellow:1, lightgray:1, lightgreen:1, lightgrey:1, lightpink:1,
            lightsalmon:1, lightseagreen:1, lightskyblue:1, lightslategray:1, lightslategrey:1,
            lightsteelblue:1, lightyellow:1, lime:1, limegreen:1, linen:1, magenta:1, maroon:1,
            mediumaquamarine:1, mediumblue:1, mediumorchid:1, mediumpurple:1, mediumseagreen:1,
            mediumslateblue:1, mediumspringgreen:1, mediumturquoise:1, mediumvioletred:1,
            midnightblue:1, mintcream:1, mistyrose:1, moccasin:1, navajowhite:1, navy:1,
            oldlace:1, olive:1, olivedrab:1, orange:1, orangered:1, orchid:1, palegoldenrod:1,
            palegreen:1, paleturquoise:1, palevioletred:1, papayawhip:1, peachpuff:1, peru:1,
            pink:1, plum:1, powderblue:1, purple:1, red:1, rosybrown:1, royalblue:1, saddlebrown:1,
            salmon:1, sandybrown:1, seagreen:1, seashell:1, sienna:1, silver:1, skyblue:1,
            slateblue:1, slategray:1, slategrey:1, snow:1, springgreen:1, steelblue:1, tan:1,
            teal:1, thistle:1, tomato:1, turquoise:1, violet:1, wheat:1, white:1, whitesmoke:1,
            yellow:1, yellowgreen:1, transparent:1
        }
        return named[s.trim().toLowerCase()] === 1
    }

    readonly property string optionsError: {
        if (!isValidColorString(textAccentField.text)) return qsTr("Text accent must be a valid color.")
        if (!isValidNumber(thresholdOkField.text)) return qsTr("OK threshold must be a number.")
        if (!isValidNumber(thresholdWarnField.text)) return qsTr("Warning threshold must be a number.")
        return ""
    }

    function hasTile(sensorId) {
        for (let i = 0; i < canvasModel.count; ++i) {
            if (canvasModel.get(i).sensorId === sensorId) return true
        }
        return false
    }

    // Bumped by every mutation of `canvasModel` (addTile, removeTile,
    // loadFromJson, Clear). The `paletteSensors` derivation reads it
    // so the filtered palette re-evaluates immediately when a tile is
    // dropped or removed — without this counter the derivation would
    // never know `canvasModel.count` changed.
    property int _canvasTick: 0

    // Palette = full sensor list MINUS anything already on the
    // canvas. Drag-drop a sensor → it disappears from the left rail;
    // remove the tile → it pops back into the rail in its
    // alphabetically-sorted slot under its category header.
    readonly property var paletteSensors: {
        const _ = page._canvasTick
        const placed = {}
        for (let i = 0; i < canvasModel.count; ++i) {
            placed[canvasModel.get(i).sensorId] = true
        }
        return page.sensors.filter(s => !placed[s.id])
    }

    // Transient feedback. Discriminated by direct type rather than the
    // previous string-prefix sniff on the message, which would have
    // garbled paths like `/home/error_user/...` and stack-confused
    // error responses inside success templates ("Saved to error: ...").
    function showSuccess(msg) {
        page.statusText = String(msg || "")
        banner.type = Kirigami.MessageType.Positive
        banner.text = page.statusText
        banner.visible = true
        bannerHideTimer.restart()
    }

    function showError(msg) {
        page.statusText = String(msg || "")
        banner.type = Kirigami.MessageType.Error
        banner.text = page.statusText
        banner.visible = true
        // We deliberately do NOT restart the hide timer for errors —
        // they stay until the next user action overwrites them.
    }

    // ----- Canvas model ------------------------------------------------------
    //
    // A simple ListModel keyed on synthetic uid; (x,y,w,h) are pixel
    // values snapped to gridStep. id ties back to a sensor in `sensors`.
    ListModel {
        id: canvasModel

        // Force-refresh path: ListModel item changes don't propagate to
        // nested bindings cleanly when only a property of the source map
        // changes, so we bump a counter that delegates depend on.
        property int valueTick: 0
        function refreshValues() { valueTick++ }
    }
    property int nextUid: 1

    // Default placement size for newly-dropped tiles. Used by `addTile`
    // and by the drop handler's centering offset so changing the size
    // here doesn't desync the visual drop position.
    readonly property int defaultTileW: 200
    readonly property int defaultTileH: 120

    function snap(v) {
        return Math.round(v / page.gridStep) * page.gridStep
    }

    function addTile(sensorId, x, y) {
        if (page.hasTile(sensorId)) {
            page.showError(qsTr("%1 is already on the canvas").arg(sensorId))
            return false
        }
        canvasModel.append({
            uid: page.nextUid++,
            sensorId: sensorId,
            x: page.snap(Math.max(0, x)),
            y: page.snap(Math.max(0, y)),
            w: page.defaultTileW,
            h: page.defaultTileH,
            options: ({}),
        })
        page._canvasTick++
        return true
    }

    function updateGeometry(index, x, y, w, h) {
        canvasModel.setProperty(index, "x", page.snap(Math.max(0, x)))
        canvasModel.setProperty(index, "y", page.snap(Math.max(0, y)))
        canvasModel.setProperty(index, "w", page.snap(Math.max(64, w)))
        canvasModel.setProperty(index, "h", page.snap(Math.max(64, h)))
    }

    function removeTile(index) {
        canvasModel.remove(index)
        page._canvasTick++
    }

    function serialize() {
        const out = []
        for (let i = 0; i < canvasModel.count; ++i) {
            const t = canvasModel.get(i)
            out.push({ id: t.sensorId, x: t.x, y: t.y, w: t.w, h: t.h, options: t.options || {} })
        }
        return JSON.stringify(out)
    }

    function loadFromJson(text) {
        try {
            const arr = JSON.parse(text)
            if (!Array.isArray(arr)) return
            canvasModel.clear()
            for (let i = 0; i < arr.length; ++i) {
                const e = arr[i]
                canvasModel.append({
                    uid: page.nextUid++,
                    sensorId: String(e.id || ""),
                    x: page.snap(Number(e.x) || 0),
                    y: page.snap(Number(e.y) || 0),
                    w: page.snap(Number(e.w) || 200),
                    h: page.snap(Number(e.h) || 120),
                    options: e.options || {},
                })
            }
            page._canvasTick++
        } catch (e) {
            page.statusText = qsTr("Load failed: %1").arg(e.toString())
        }
    }

    // ----- Layout ------------------------------------------------------------

    // Transient banner. Floats below the header; success-type entries
    // auto-hide after 4 s, error-type entries stick.
    Kirigami.InlineMessage {
        id: banner
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: app.tokens.spaceL
        anchors.rightMargin: app.tokens.spaceL
        anchors.topMargin: app.tokens.spaceS
        visible: false
        showCloseButton: true
        z: 10
    }
    Timer {
        id: bannerHideTimer
        interval: 4000
        repeat: false
        onTriggered: {
            if (banner.type !== Kirigami.MessageType.Error) {
                banner.visible = false
            }
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
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: app.tokens.spaceXL
            anchors.rightMargin: app.tokens.spaceXL
            spacing: app.tokens.spaceM
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1
                Controls.Label {
                    text: page.editingName.length > 0
                          ? qsTr("Editor — %1").arg(page.editingName)
                          : qsTr("Editor")
                    font.pixelSize: app.tokens.textHeading
                    font.weight: app.tokens.weightBold
                    font.family: app.tokens.sansFamily
                    color: app.tokens.textPrimary
                }
                Controls.Label {
                    text: qsTr("Drag sensors from the left rail onto the canvas. Resize from the bottom-right corner.")
                    opacity: 0.6
                    font.pixelSize: app.tokens.textCaption + 1
                    color: app.tokens.textPrimary
                }
            }
            Controls.ToolButton {
                visible: page.editingSlug.length > 0
                icon.name: "view-more-symbolic"
                Controls.ToolTip.text: qsTr("Dashboard actions")
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 400
                Accessible.name: Controls.ToolTip.text
                onClicked: dashboardActionsMenu.open()

                Controls.Menu {
                    id: dashboardActionsMenu
                    Controls.MenuItem {
                        text: qsTr("Rename…")
                        icon.name: "edit-rename-symbolic"
                        onTriggered: renameDialog.open()
                    }
                    Controls.MenuItem {
                        text: qsTr("Duplicate")
                        icon.name: "edit-copy-symbolic"
                        onTriggered: {
                            const ns = app.dashboards.duplicate(page.editingSlug).toString()
                            if (ns.length > 0) {
                                app.goTo("editor:" + ns)
                            } else {
                                page.showError(app.dashboards.lastError
                                               || qsTr("Duplicate failed."))
                            }
                        }
                    }
                    Controls.MenuItem {
                        text: qsTr("Delete…")
                        icon.name: "edit-delete-symbolic"
                        onTriggered: deleteDialog.open()
                    }
                }
            }
        }
    }

    // Inline rename / delete dialogs scoped to this page so they share
    // the editor's slug context without needing extra plumbing through
    // Main.qml.
    Controls.Dialog {
        id: renameDialog
        title: qsTr("Rename Dashboard")
        modal: true
        standardButtons: Controls.Dialog.Cancel | Controls.Dialog.Ok
        width: 380
        anchors.centerIn: parent
        onAboutToShow: {
            renameField.text = page.editingName
            renameField.forceActiveFocus()
            renameField.selectAll()
        }
        onAccepted: {
            const t = renameField.text.replace(/^\s+|\s+$/g, "")
            if (t.length === 0 || t === page.editingName) return
            const ns = app.dashboards
                .rename(page.editingSlug, t).toString()
            if (ns.length === 0) {
                page.showError(app.dashboards.lastError
                               || qsTr("Rename failed."))
                return
            }
            if (ns !== page.editingSlug) {
                // Slug shifted because the name normalized to a new
                // filesystem slug — repoint navigation at the new file.
                app.goTo("editor:" + ns)
            } else {
                // Name changed but slug didn't; refresh the header
                // immediately rather than waiting for a notify tick.
                page._nameTick++
            }
        }
        contentItem: ColumnLayout {
            spacing: app.tokens.spaceS
            Controls.TextField {
                id: renameField
                Layout.fillWidth: true
                Keys.onReturnPressed: if (text.length > 0) renameDialog.accept()
                Keys.onEnterPressed:  if (text.length > 0) renameDialog.accept()
            }
        }
    }

    Controls.Dialog {
        id: deleteDialog
        title: qsTr("Delete Dashboard?")
        modal: true
        standardButtons: Controls.Dialog.Cancel | Controls.Dialog.Discard
        width: 420
        anchors.centerIn: parent
        onDiscarded: {
            const removed = app.dashboards.remove(page.editingSlug)
            if (!removed) {
                page.showError(app.dashboards.lastError
                               || qsTr("Delete failed."))
                return
            }
            // Reset Start Page if it pointed at the deleted slug —
            // otherwise next launch would resolve the saved
            // `dashboard:<slug>` to nothing and fall back to Overview
            // silently. The Settings dropdown should reflect reality
            // before the user has a chance to re-open it.
            if (app.preferences
                && app.preferences.startPage === "dashboard:" + page.editingSlug) {
                app.preferences.applyStartPage("overview")
            }
            try {
                const list = JSON.parse(app.dashboards.summaryJson || "[]")
                if (list.length > 0) {
                    app.goTo("editor:" + String(list[0].slug))
                    return
                }
            } catch (e) {}
            app.goTo("overview")
        }
        contentItem: Controls.Label {
            text: qsTr("\"%1\" will be permanently deleted from disk.").arg(page.editingName)
            wrapMode: Text.WordWrap
        }
    }

    RowLayout {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        spacing: 0

        // ---- Left rail: palette --------------------------------------------
        Rectangle {
            Layout.preferredWidth: 240
            Layout.fillHeight: true
            color: app.tokens.surface1
            Rectangle {
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: 1
                color: app.tokens.separator
            }
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: app.tokens.spaceL
                spacing: app.tokens.spaceS

                Controls.Label {
                    text: qsTr("SENSORS")
                    font.pixelSize: 10
                    font.weight: app.tokens.weightSemibold
                    opacity: 0.5
                    color: app.tokens.textPrimary
                }
                Controls.ScrollView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    contentWidth: availableWidth
                    ListView {
                        id: paletteList
                        // `paletteSensors` = full catalogue minus
                        // anything currently on the canvas; dropping
                        // a sensor removes it from the rail, removing
                        // the tile pops it back into the sorted slot.
                        model: page.paletteSensors
                        spacing: 4
                        cacheBuffer: 5 * 44

                        // Sectioned by category. `refreshSensors`
                        // sorts the model by (categoryRank, name) so
                        // each category appears as one contiguous
                        // run; the section delegate paints a sticky
                        // header above each run.
                        section.property: "category"
                        section.criteria: ViewSection.FullString
                        section.delegate: Rectangle {
                            width: paletteList.width
                            height: 24
                            color: app.tokens.surface1
                            Controls.Label {
                                anchors.left: parent.left
                                anchors.leftMargin: app.tokens.spaceS
                                anchors.verticalCenter: parent.verticalCenter
                                text: section
                                font.pixelSize: 10
                                font.weight: app.tokens.weightSemibold
                                font.capitalization: Font.AllUppercase
                                color: app.tokens.textPrimary
                                opacity: 0.6
                            }
                        }

                        delegate: PaletteRow {
                            width: paletteList.width
                            sensorId: modelData.id
                            sensorName: modelData.name
                            sensorDeviceLabel: modelData.deviceLabel || ""
                            // Live value via valueById lookup, NOT a
                            // model field — keeps the model reference
                            // stable so scroll position is preserved
                            // across sample ticks.
                            sensorValue: page.valueById[modelData.id] || "…"
                        }
                    }
                }
            }
        }

        // ---- Center: canvas -------------------------------------------------
        Rectangle {
            id: canvasFrame
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: app.tokens.surface0

            // 8px snap grid background. A repeating Canvas-painted dot
            // matrix would look fancier but Canvas in a scrollable
            // viewport tanks frame rate on modest GPUs; tiled rect is
            // virtually free.
            Item {
                anchors.fill: parent
                clip: true
                Repeater {
                    model: Math.ceil(canvasFrame.width / 64)
                    delegate: Rectangle {
                        x: index * 64
                        y: 0
                        width: 1
                        height: canvasFrame.height
                        color: app.tokens.separator
                        opacity: 0.35
                    }
                }
                Repeater {
                    model: Math.ceil(canvasFrame.height / 64)
                    delegate: Rectangle {
                        x: 0
                        y: index * 64
                        width: canvasFrame.width
                        height: 1
                        color: app.tokens.separator
                        opacity: 0.35
                    }
                }
            }

            // Drop area: drops outside an existing tile create a new
            // tile at the drop point.
            //
            // We use `Drag.Internal` in the palette (see PaletteRow
            // comment for why), so the drop event has no MIME data —
            // the sensor ID is read from `drop.source.sensorId` on
            // the source proxy. The `keys` filter still routes the
            // drop correctly because Internal mode honors Drag.keys.
            DropArea {
                anchors.fill: parent
                keys: ["application/x-linsight-sensor-id"]
                onDropped: (drop) => {
                    const sid = (drop.source && drop.source.sensorId) || ""
                    if (sid.length > 0) {
                        if (page.addTile(sid, drop.x - page.defaultTileW / 2, drop.y - page.defaultTileH / 2)) {
                            drop.accept(Qt.CopyAction)
                        } else {
                            drop.accepted = false
                        }
                    }
                }
            }

            Repeater {
                model: canvasModel
                delegate: CanvasTile {
                    uidValue: model.uid
                    sensorId: model.sensorId
                    tileX: model.x
                    tileY: model.y
                    tileW: model.w
                    tileH: model.h
                    tileOptions: model.options || ({})
                    onCommitGeometry: (nx, ny, nw, nh) =>
                        page.updateGeometry(index, nx, ny, nw, nh)
                    onRemoveRequested: page.removeTile(index)
                    onOptionsRequested: page.openOptionsFor(index)
                }
            }

            Controls.Label {
                anchors.centerIn: parent
                visible: canvasModel.count === 0
                text: qsTr("Drop sensors here to build your dashboard")
                opacity: 0.5
                color: app.tokens.textPrimary
                font.pixelSize: app.tokens.textBody
            }
        }

        // ---- Right strip: save / load / status -----------------------------
        Rectangle {
            Layout.preferredWidth: 240
            Layout.fillHeight: true
            color: app.tokens.surface1
            Rectangle {
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: 1
                color: app.tokens.separator
            }
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: app.tokens.spaceL
                spacing: app.tokens.spaceM

                Controls.Label {
                    text: qsTr("LAYOUT")
                    font.pixelSize: 10
                    font.weight: app.tokens.weightSemibold
                    opacity: 0.5
                    color: app.tokens.textPrimary
                }
                Controls.Button {
                    Layout.fillWidth: true
                    icon.name: "document-save-symbolic"
                    text: qsTr("Save")
                    // Saving an empty dashboard is a legitimate user
                    // action ("clear, then keep"); a disabled Save
                    // would resurrect the old tiles on the next
                    // reload. The slug must be valid, nothing more.
                    enabled: page.editingSlug.length > 0 && app.dashboards
                    onClicked: {
                        if (!app.dashboards || page.editingSlug.length === 0) return
                        const result = app.dashboards
                            .saveLayout(page.editingSlug, page.serialize())
                            .toString()
                        if (result.length === 0) {
                            page.showError(app.dashboards.lastError
                                          || qsTr("Save failed."))
                        } else {
                            page.showSuccess(qsTr("Saved %1 (%2 tile(s))")
                                .arg(page.editingName).arg(canvasModel.count))
                        }
                    }
                }
                Controls.Button {
                    Layout.fillWidth: true
                    icon.name: "document-open-symbolic"
                    text: qsTr("Reload")
                    enabled: page.editingSlug.length > 0 && app.dashboards
                    onClicked: {
                        page.autoLoadFromSlug()
                        page.showSuccess(qsTr("Reloaded %1 tile(s) from %2")
                            .arg(canvasModel.count).arg(page.editingName))
                    }
                }
                Controls.Button {
                    Layout.fillWidth: true
                    icon.name: "edit-clear-symbolic"
                    text: qsTr("Clear")
                    onClicked: { canvasModel.clear(); page._canvasTick++ }
                }

                Item { Layout.preferredHeight: app.tokens.spaceL }

                Controls.Label {
                    text: qsTr("STATUS")
                    font.pixelSize: 10
                    font.weight: app.tokens.weightSemibold
                    opacity: 0.5
                    color: app.tokens.textPrimary
                }
                Controls.Label {
                    Layout.fillWidth: true
                    text: page.statusText
                    wrapMode: Text.WrapAnywhere
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    opacity: 0.75
                    color: app.tokens.textPrimary
                }

                Item { Layout.fillHeight: true }

                Controls.Label {
                    Layout.fillWidth: true
                    text: qsTr("%1 tile(s)").arg(canvasModel.count)
                    font.pixelSize: app.tokens.textCaption
                    opacity: 0.6
                    color: app.tokens.textPrimary
                    horizontalAlignment: Text.AlignRight
                }
            }
        }
    }

    // ----- Inline components -------------------------------------------------

    // PaletteRow: draggable proxy row in the left rail.
    //
    // Uses `Drag.Internal`, NOT `Drag.Automatic`. The Automatic
    // variant starts a Qt MIME-level drag (QDrag) synchronously the
    // moment `Drag.active` flips true. Setting that inside a
    // MouseArea.onPressed handler crashes the GUI on Wayland — the
    // compositor's input grab races the toolkit's drag grab.
    // Internal mode keeps the drag entirely in the Qt event loop —
    // no compositor handoff — and works reliably with `drag.target`
    // for visual following. The `DropArea` on the canvas reads
    // `drag.source.sensorId` because Internal mode doesn't pass MIME
    // data; the `Drag.keys` filter routes the drop correctly.
    component PaletteRow : Item {
        id: row
        property string sensorId: ""
        property string sensorName: ""
        property string sensorDeviceLabel: ""
        property string sensorValue: ""
        // Grow when a device line is shown so multiple same-metric devices
        // (e.g. two GPUs) stay distinguishable in the picker.
        height: sensorDeviceLabel.length > 0 ? 56 : 44

        Rectangle {
            anchors.fill: parent
            anchors.margins: 2
            radius: app.tokens.radiusInput
            color: dragArea.containsMouse ? app.tokens.surface2 : "transparent"
            border.color: app.tokens.separator
            border.width: 1

            ColumnLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceS
                anchors.rightMargin: app.tokens.spaceS
                spacing: 0
                Controls.Label {
                    text: row.sensorName
                    font.pixelSize: app.tokens.textBody
                    color: app.tokens.textPrimary
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                Controls.Label {
                    visible: row.sensorDeviceLabel.length > 0
                    text: row.sensorDeviceLabel
                    font.pixelSize: app.tokens.textCaption
                    opacity: 0.55
                    color: app.tokens.textPrimary
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                Controls.Label {
                    text: row.sensorValue
                    font.pixelSize: app.tokens.textCaption
                    font.family: app.tokens.monoFamily
                    opacity: 0.6
                    color: app.tokens.textPrimary
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                }
            }
        }

        // Visual drag proxy: a translucent floating pill that follows
        // the cursor via `drag.target`. Parented to `page` so it can
        // float over the canvas during the drag without being clipped
        // by the palette ScrollView. Because the proxy lives in a
        // DIFFERENT coordinate space from the MouseArea, we have to
        // seed its initial position via `mapToItem` in `onPressed` —
        // otherwise the MouseArea's drag controller translates from
        // the proxy's default (0,0) and the visible pill drifts up
        // into the top-left of the page rather than following the
        // cursor.
        Item {
            id: dragProxy
            parent: page
            width: 160
            height: 36
            visible: Drag.active
            opacity: 0.85
            // Carry the sensor ID via a property; the DropArea reads
            // it from `drop.source.sensorId` on drop.
            property string sensorId: row.sensorId
            Rectangle {
                anchors.fill: parent
                radius: app.tokens.radiusCard
                color: app.tokens.accentMute
                border.color: app.tokens.accent
                border.width: 1
                Controls.Label {
                    anchors.centerIn: parent
                    text: row.sensorName
                    color: app.tokens.textPrimary
                    font.pixelSize: app.tokens.textCaption + 1
                }
            }
            Drag.hotSpot.x: width / 2
            Drag.hotSpot.y: height / 2
            Drag.dragType: Drag.Internal
            Drag.keys: ["application/x-linsight-sensor-id"]
            Drag.supportedActions: Qt.CopyAction
        }

        MouseArea {
            id: dragArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.OpenHandCursor
            drag.target: dragProxy
            onPressed: (mouse) => {
                // Project the press point into `page` coords (the
                // proxy's parent). Center the proxy on the cursor so
                // subsequent drag-translation tracks the mouse 1:1.
                const p = dragArea.mapToItem(page, mouse.x, mouse.y)
                dragProxy.x = p.x - dragProxy.width / 2
                dragProxy.y = p.y - dragProxy.height / 2
                // Manually arm the drag so the DropArea sees an
                // active source even before the threshold is met.
                // (Safe on Wayland — Drag.Internal does NOT call
                // QDrag::exec; only Automatic mode does.)
                dragProxy.Drag.active = true
            }
            onReleased: {
                // Trigger the dropped() signal on the DropArea under
                // the cursor BEFORE deactivating the drag. Without
                // the explicit Drag.drop() call here the DropArea
                // never fires.
                dragProxy.Drag.drop()
                dragProxy.Drag.active = false
                // Cosmetic snap-back so the next drag from the same
                // row starts cleanly.
                dragProxy.x = 0
                dragProxy.y = 0
            }
        }
    }

    // CanvasTile: a placed sensor tile. Free-positioned, draggable by
    // header, resizable from a corner grip.
    component CanvasTile : Rectangle {
        id: tile
        property int uidValue: 0
        property string sensorId: ""
        property int tileX: 0
        property int tileY: 0
        property int tileW: 200
        property int tileH: 120
        property var tileOptions: ({})
        signal commitGeometry(int nx, int ny, int nw, int nh)
        signal removeRequested()
        signal optionsRequested()

        function commitSnappedGeometry() {
            const snappedX = page.snap(Math.max(0, tile.x))
            const snappedY = page.snap(Math.max(0, tile.y))
            const snappedW = page.snap(Math.max(64, tile.width))
            const snappedH = page.snap(Math.max(64, tile.height))
            tile.x = snappedX
            tile.y = snappedY
            tile.width = snappedW
            tile.height = snappedH
            tile.commitGeometry(snappedX, snappedY, snappedW, snappedH)
        }

        x: tileX
        y: tileY
        width: tileW
        height: tileH
        radius: app.tokens.radiusCard
        color: app.tokens.surface1
        border.color: app.tokens.separator
        border.width: 1

        // Cause re-evaluation of liveValue when the model bumps its tick.
        readonly property int _valueTick: canvasModel.valueTick
        readonly property string liveValue: {
            // Touch the tick so the binding re-runs.
            void tile._valueTick
            const m = page.valueById
            return (m && m[tile.sensorId] !== undefined) ? m[tile.sensorId] : "…"
        }
        readonly property string displayName: {
            for (let i = 0; i < page.sensors.length; ++i) {
                if (page.sensors[i].id === tile.sensorId) return page.sensors[i].name
            }
            return tile.sensorId
        }
        // Resolved device label (nickname || model) for the second title
        // line. Empty for non-device sensors.
        readonly property string deviceLabel: {
            for (let i = 0; i < page.sensors.length; ++i) {
                if (page.sensors[i].id === tile.sensorId) return page.sensors[i].deviceLabel || ""
            }
            return ""
        }

        // Drag handle: header strip. Body is non-interactive so the
        // resize grip in the bottom-right stays accessible.
        Rectangle {
            id: headerStrip
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            // Grows to fit the stacked title (metric + device label). A
            // single-line title (no device) keeps the original 22px strip.
            height: Math.max(22, titleCol.implicitHeight + 4)
            radius: app.tokens.radiusCard
            color: app.tokens.surface2
            // Square off the bottom corners so the strip blends into
            // the tile body cleanly.
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: parent.radius
                color: parent.color
            }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceS
                anchors.rightMargin: 2
                spacing: 0
                ColumnLayout {
                    id: titleCol
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    spacing: 0
                    Controls.Label {
                        Layout.fillWidth: true
                        text: tile.displayName
                        font.pixelSize: app.tokens.textCaption
                        font.weight: app.tokens.weightSemibold
                        color: app.tokens.textPrimary
                        elide: Text.ElideRight
                    }
                    // Second line: hardware device (nickname || model).
                    // visible:false collapses it out of the layout, so a
                    // non-device tile keeps a single-line header.
                    Controls.Label {
                        visible: tile.deviceLabel.length > 0
                        Layout.fillWidth: true
                        text: tile.deviceLabel
                        font.pixelSize: app.tokens.textCaption
                        color: app.tokens.textPrimary
                        opacity: 0.55
                        elide: Text.ElideRight
                    }
                }
                Controls.ToolButton {
                    icon.name: "configure-symbolic"
                    icon.width: 12
                    icon.height: 12
                    implicitWidth: 22
                    implicitHeight: 22
                    onClicked: tile.optionsRequested()
                }
                Controls.ToolButton {
                    icon.name: "window-close-symbolic"
                    icon.width: 12
                    icon.height: 12
                    implicitWidth: 22
                    implicitHeight: 22
                    onClicked: tile.removeRequested()
                }
            }

            MouseArea {
                anchors.fill: parent
                anchors.rightMargin: 24  // keep close button clickable
                cursorShape: Qt.SizeAllCursor

                // Native Qt drag. `drag.target: tile` lets the
                // MouseArea drive the tile's x/y at the frame rate
                // of the render thread — no per-event onPositionChanged
                // round-trip through JS, no manual mapToItem math
                // (Qt internally tracks the press offset). The
                // previous JS-driven path produced visible cursor↔tile
                // lag during fast drags because every move had to
                // pass through the QML event loop before the tile
                // moved.
                drag.target: tile
                drag.axis: Drag.XAndYAxis
                drag.minimumX: 0
                drag.minimumY: 0
                drag.smoothed: false
                onPressed: page._isDragging = true
                onReleased: {
                    page._isDragging = false
                    tile.commitSnappedGeometry()
                }
                onCanceled: page._isDragging = false
            }
        }

        // Body — sensor value or table, mirroring SensorTile.
        Loader {
            id: canvasBodyLoader
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: headerStrip.bottom
            anchors.bottom: parent.bottom
            anchors.margins: app.tokens.spaceS
            sourceComponent: {
                var kind = page.kindById[tile.sensorId] || "scalar"
                if (kind === "table" && page.rowsById[tile.sensorId]) return canvasTableComp
                return canvasScalarComp
            }
            property string _sid: tile.sensorId
        }

        // Resize grip — bottom-right corner.
        Rectangle {
            id: grip
            width: 14
            height: 14
            radius: 3
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: 2
            color: gripArea.containsMouse ? app.tokens.accent : app.tokens.separator
            opacity: 0.9
            MouseArea {
                id: gripArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.SizeFDiagCursor
                // Same coordinate-frame fix as the body drag: the
                // grip is anchored to the bottom-right of the tile,
                // so when the tile resizes the grip moves and the
                // MouseArea's origin shifts under the cursor. Map
                // into the tile's parent (canvasFrame) for stable
                // deltas.
                property real anchorX: 0
                property real anchorY: 0
                property real startW: 0
                property real startH: 0
                onPressed: (mouse) => {
                    page._isDragging = true
                    const p = mapToItem(tile.parent, mouse.x, mouse.y)
                    anchorX = p.x
                    anchorY = p.y
                    startW = tile.width
                    startH = tile.height
                }
                onPositionChanged: (mouse) => {
                    if (!pressed) return
                    const p = mapToItem(tile.parent, mouse.x, mouse.y)
                    tile.width = Math.max(64, startW + (p.x - anchorX))
                    tile.height = Math.max(64, startH + (p.y - anchorY))
                }
                onReleased: {
                    page._isDragging = false
                    tile.commitSnappedGeometry()
                }
                onCanceled: page._isDragging = false
            }
        }
    }

    Component {
        id: canvasScalarComp
        Controls.Label {
            text: tile.liveValue
            font.pixelSize: app.tokens.textSubheading + 4
            font.weight: app.tokens.weightMedium
            color: app.tokens.textPrimary
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
    }

    Component {
        id: canvasTableComp
        Controls.ScrollView {
            clip: true
            Controls.ScrollBar.horizontal.policy: Controls.ScrollBar.AlwaysOff
            ListView {
                model: page.rowsById[_sid] || []
                interactive: true
                boundsBehavior: Flickable.StopAtBounds
                delegate: RowLayout {
                    width: availableWidth
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

    property int _optionsIndex: -1

    function openOptionsFor(index) {
        page._optionsIndex = index
        const t = canvasModel.get(index)
        if (!t) return
        const opts = t.options || {}
        labelOverrideField.text = opts.labelOverride || ""
        textAccentField.text = opts.textAccent || ""
        conditionField.text = opts.condition || ""
        thresholdCheck.checked = !!opts.thresholdEnabled
        thresholdOkField.text = opts.thresholdOk !== undefined ? String(opts.thresholdOk) : ""
        thresholdWarnField.text = opts.thresholdWarn !== undefined ? String(opts.thresholdWarn) : ""
        optionsDrawer.open()
    }

    function applyOptions() {
        if (page._optionsIndex < 0) return
        if (page.optionsError.length > 0) {
            app.showPassiveNotification(page.optionsError, 4000)
            return
        }
        const opts = {}
        if (labelOverrideField.text.trim()) opts.labelOverride = labelOverrideField.text.trim()
        if (textAccentField.text.trim()) opts.textAccent = textAccentField.text.trim()
        if (conditionField.text.trim()) opts.condition = conditionField.text.trim()
        if (thresholdCheck.checked) {
            opts.thresholdEnabled = true
            if (thresholdOkField.text.trim()) opts.thresholdOk = Number(thresholdOkField.text.trim())
            if (thresholdWarnField.text.trim()) opts.thresholdWarn = Number(thresholdWarnField.text.trim())
        }
        canvasModel.setProperty(page._optionsIndex, "options", opts)
        optionsDrawer.close()
    }

    Kirigami.OverlayDrawer {
        id: optionsDrawer
        edge: Qt.RightEdge
        width: 300
        height: parent.height
        modal: true
        closePolicy: Kirigami.OverlayDrawer.CloseOnEscape | Kirigami.OverlayDrawer.CloseOnPressOutside

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: app.tokens.spaceM
            spacing: app.tokens.spaceM

            Controls.Label {
                text: qsTr("Tile Options")
                font.pixelSize: app.tokens.textBody
                font.weight: app.tokens.weightBold
                color: app.tokens.textPrimary
            }

            Controls.Label {
                text: qsTr("Label Override")
                font.pixelSize: app.tokens.textCaption
                opacity: 0.7
                color: app.tokens.textPrimary
            }
            Controls.TextField {
                id: labelOverrideField
                Layout.fillWidth: true
                placeholderText: qsTr("Custom display name")
            }

            Controls.Label {
                text: qsTr("Text Accent Color")
                font.pixelSize: app.tokens.textCaption
                opacity: 0.7
                color: app.tokens.textPrimary
            }
            Controls.TextField {
                id: textAccentField
                Layout.fillWidth: true
                placeholderText: qsTr("e.g. #4fc3f7")
            }

            Controls.Label {
                text: qsTr("Visibility Condition")
                font.pixelSize: app.tokens.textCaption
                opacity: 0.7
                color: app.tokens.textPrimary
            }
            Controls.TextField {
                id: conditionField
                Layout.fillWidth: true
                placeholderText: qsTr("e.g. cpu.util > 50")
            }

            Controls.CheckBox {
                id: thresholdCheck
                text: qsTr("Enable Threshold Colors")
            }

            Controls.Label {
                text: qsTr("OK Threshold")
                font.pixelSize: app.tokens.textCaption
                opacity: 0.7
                color: app.tokens.textPrimary
                visible: thresholdCheck.checked
            }
            Controls.TextField {
                id: thresholdOkField
                Layout.fillWidth: true
                placeholderText: qsTr("e.g. 50")
                visible: thresholdCheck.checked
            }

            Controls.Label {
                text: qsTr("Warning Threshold")
                font.pixelSize: app.tokens.textCaption
                opacity: 0.7
                color: app.tokens.textPrimary
                visible: thresholdCheck.checked
            }
            Controls.TextField {
                id: thresholdWarnField
                Layout.fillWidth: true
                placeholderText: qsTr("e.g. 80")
                visible: thresholdCheck.checked
            }

            Item { Layout.fillHeight: true }

            Controls.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                visible: page.optionsError.length > 0
                text: page.optionsError
                color: app.tokens.negative
                font.pixelSize: app.tokens.textCaption
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceS
                Controls.Button {
                    text: qsTr("Apply")
                    Layout.fillWidth: true
                    enabled: page.optionsError.length === 0
                    onClicked: page.applyOptions()
                }
                Controls.Button {
                    text: qsTr("Cancel")
                    flat: true
                    Layout.fillWidth: true
                    onClicked: optionsDrawer.close()
                }
            }
        }
    }

}
