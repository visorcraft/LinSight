// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    title: pageTitle
    padding: 0

    property string category: ""
    property string pageTitle: ""
    property QtObject dashModel: null
    property string groupBy: ""

    Accessible.role: Accessible.Pane
    Accessible.name: pageTitle

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    // Filter the shared tilesJson on every change. The tilesJsonChanged
    // NOTIFY makes this binding re-eval whenever the model gets fresh
    // samples.
    //
    // Note: the "-1" filter is a workaround for sensors that use -1 as
    // an "unknown" sentinel (e.g. net.speed_mbps when the kernel writes
    // -1 for a virtual interface). We only suppress the EXACT scalar
    // form "-1" (post-formatter); earlier code did `indexOf("-1") === 0`
    // which also hid legitimate "-1.0°C" / "-1 rpm" readings on a
    // genuinely cold or stopped device. Long-term this should become an
    // explicit `available: bool` field on the tile JSON; see
    // `format_reading` in `overview_model.rs`.
    readonly property var tilesArray: {
        if (!page.dashModel) return []
        try {
            const raw = JSON.parse(page.dashModel.tilesJson || "[]")
            let filtered = raw.filter(t => t.category === page.category
                && !(typeof t.value === "string" && page.isUnknownSentinel(t.value)))

            if (page.groupBy === "") return filtered

            if (page.category === "gpu" && page.groupBy === "deviceLabel") {
                // GPU-specific sorting: order groups by Total RAM descending,
                // with no-RAM groups at the end.
                const groups = new Map()
                for (const t of filtered) {
                    const key = t[page.groupBy] || ""
                    if (!groups.has(key)) groups.set(key, [])
                    groups.get(key).push(t)
                }

                const groupKeys = Array.from(groups.keys()).sort((a, b) => {
                    const getSortValue = (key) => {
                        const tiles = groups.get(key)
                        // Match the total-VRAM sensor by its id suffix, which
                        // is vendor-independent (nvml/amdgpu/xe sensor ids all
                        // end in ".mem_total_bytes") and survives display-name
                        // changes — every vendor's tile now shows "GPU VRAM
                        // total", but matching the id keeps this robust.
                        const totalRamTile = tiles.find(t => (t.id || "").endsWith("mem_total_bytes"))
                        if (!totalRamTile) return -1

                        const match = totalRamTile.value.match(/^(\d+(\.\d+)?)\s*(TB|TiB|GB|GiB|MB|MiB|KB|KiB|B)$/)
                        if (match) {
                            let val = parseFloat(match[1])
                            const unit = match[3]
                            if (unit === "TB" || unit === "TiB") val *= 1024
                            else if (unit === "GB" || unit === "GiB") val *= 1
                            else if (unit === "MB" || unit === "MiB") val /= 1024
                            else if (unit === "KB" || unit === "KiB") val /= (1024 * 1024)
                            else if (unit === "B") val /= (1024 * 1024 * 1024)
                            return val
                        }
                        return -1
                    }

                    const valA = getSortValue(a)
                    const valB = getSortValue(b)
                    if (valA !== valB) return valB - valA
                    return a.localeCompare(b)
                })

                const result = []
                for (const key of groupKeys) {
                    result.push({ type: "header", label: key })
                    const tiles = groups.get(key)
                    tiles.sort((a, b) => (a.name || "").localeCompare(b.name || ""))
                    for (const t of tiles) {
                        result.push(Object.assign({}, t, { type: "tile" }))
                    }
                }
                return result
            }

            filtered.sort((a, b) => {
                const valA = a[page.groupBy] || ""
                const valB = b[page.groupBy] || ""
                if (valA !== valB) return valA.localeCompare(valB)
                return (a.name || "").localeCompare(b.name || "")
            })

            const result = []
            let lastGroup = null
            for (const t of filtered) {
                const currentGroup = t[page.groupBy] || ""
                if (currentGroup !== lastGroup) {
                    result.push({ type: "header", label: currentGroup })
                    lastGroup = currentGroup
                }
                result.push(Object.assign({}, t, { type: "tile" }))
            }
            return result
        } catch (e) {
            return []
        }
    }

    function isUnknownSentinel(v) {
        // Match the formatter's exact output for the kernel's -1 sentinel
        // across the units sensors actually emit it for. Anything else
        // (including "-1.0°C" or "-1.5%") is a real reading.
        return v === "-1" || v === "-1 rpm" || v === "-1 Hz" || v === "-1 B/s"
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
                text: page.pageTitle
                font.pixelSize: app.tokens.textHeading
                font.weight: app.tokens.weightBold
                font.family: app.tokens.sansFamily
            }
            Controls.Label {
                // Two separate qsTr strings, not `%1 sensor%2` with a
                // suffix arg — that pattern doesn't translate into German
                // ("1 Sensor" / "2 Sensoren"), Japanese (no plural), or
                // most other languages. Each form gets its own
                // translation key.
                text: page.tilesArray.length === 1
                    ? qsTr("%1 sensor").arg(1)
                    : qsTr("%1 sensors").arg(page.tilesArray.length)
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

        GridLayout {
            id: grid
            width: parent.width
            columns: Math.max(1, Math.floor(parent.width / 240))
            rowSpacing: app.tokens.spaceM
            columnSpacing: app.tokens.spaceM

            Repeater {
                model: page.tilesArray
                delegate: Loader {
                    id: cellLoader
                    sourceComponent: modelData.type === "header" ? headerComponent : tileComponent
                    Layout.columnSpan: modelData.type === "header" ? grid.columns : 1
                    Layout.fillWidth: true
                    Layout.preferredHeight: modelData.type === "header" ? 32 : (modelData.kind === "table" && modelData.rows && modelData.rows.length > 0 ? 280 : 156)
                    onLoaded: {
                        item.anchors.fill = cellLoader
                        if (modelData.type === "header") {
                            item.label = modelData.label
                        } else {
                            item.tileName = modelData.name
                            item.tileDeviceLabel = modelData.deviceLabel || ""
                            item.tileValue = modelData.value
                            item.tileKind = modelData.kind || "scalar"
                            item.tileRows = modelData.rows || []
                        }
                    }
                }
            }

            Component {
                id: headerComponent
                Rectangle {
                    property string label: ""
                    Layout.fillWidth: true
                    color: app.tokens.surface0

                    Rectangle {
                        anchors.bottom: parent.bottom
                        width: parent.width
                        height: 1
                        color: app.tokens.separator
                    }

                    Controls.Label {
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left
                        anchors.leftMargin: app.tokens.spaceM
                        text: label
                        font.pixelSize: app.tokens.textCaption
                        font.weight: app.tokens.weightBold
                        opacity: 0.6
                    }
                }
            }

            Component {
                id: tileComponent
                SensorTile {
                    Layout.fillWidth: true
                }
            }
        }
    }

    Controls.Label {
        anchors.centerIn: parent
        text: qsTr("No %1 sensors detected").arg(page.category)
        visible: page.tilesArray.length === 0
        opacity: 0.55
        font.pixelSize: app.tokens.textBody
    }
}
