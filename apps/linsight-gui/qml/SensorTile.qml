// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Rectangle {
    id: root

    property string tileName: "…"
    // Resolved hardware device label (nickname || model). Rendered as a
    // dimmer secondary line under tileName; empty for non-device sensors.
    property string tileDeviceLabel: ""
    property string tileValue: "…"
    property string tileKind: "scalar"
    property var tileRows: []
    property var tileOptions: ({})
    property var tileSparkline: []

    property real thresholdOk: 50.0
    property real thresholdWarn: 80.0

    color: app.tokens.surface1
    radius: app.tokens.radiusCard

    // Border: static width + color by default, but threshold-based when
    // the tile's options enable it. Binding to a function keeps QML from
    // complaining about duplicate property writes.
    readonly property real __borderWidth: {
        if (!tileOptions || !tileOptions.thresholdEnabled) return 1;
        var numVal = parseFloat(tileValue);
        if (isNaN(numVal)) return 1;
        if (tileOptions.thresholdWarn && numVal >= parseFloat(tileOptions.thresholdWarn)) return 2;
        if (tileOptions.thresholdOk && numVal >= parseFloat(tileOptions.thresholdOk)) return 2;
        return 1;
    }
    readonly property color __borderColor: {
        if (!tileOptions || !tileOptions.thresholdEnabled) return app.tokens.separator;
        var numVal = parseFloat(tileValue);
        if (isNaN(numVal)) return app.tokens.separator;
        if (tileOptions.thresholdWarn && numVal >= parseFloat(tileOptions.thresholdWarn)) return Kirigami.Theme.negativeTextColor;
        if (tileOptions.thresholdOk && numVal >= parseFloat(tileOptions.thresholdOk)) return Kirigami.Theme.warningTextColor;
        return app.tokens.separator;
    }
    border.color: root.__borderColor
    border.width: root.__borderWidth

    // True only when the sparkline series actually varies. A constant value
    // (e.g. GPU memory total) gets no chart — it has no trend, and a flat
    // line otherwise consumed the value label's layout space.
    readonly property bool __sparklineVaries: {
        const pts = root.tileSparkline
        if (!Array.isArray(pts) || pts.length < 2) return false
        let mn = pts[0], mx = pts[0]
        for (let k = 1; k < pts.length; ++k) {
            if (pts[k] < mn) mn = pts[k]
            if (pts[k] > mx) mx = pts[k]
        }
        return mx > mn
    }

    Accessible.role: Accessible.Indicator
    Accessible.name: root.tileDeviceLabel.length > 0
                     ? (root.tileDeviceLabel + " " + root.tileName)
                     : root.tileName
    Accessible.description: root.tileName + " " + root.tileValue
    Accessible.ignored: false

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.smallSpacing

        // Header row: name + optional label override
        Label {
            text: (tileOptions && tileOptions.labelOverride) ? tileOptions.labelOverride : root.tileName
            color: (tileOptions && tileOptions.textAccent) ? tileOptions.textAccent : app.tokens.textPrimary
            opacity: 0.7
            font.pixelSize: Kirigami.Theme.defaultFont.pixelSize
            font.weight: Font.DemiBold
            font.capitalization: Font.AllUppercase
            Layout.fillWidth: true
            Accessible.ignored: true
        }

        // Secondary line: the hardware device this metric belongs to
        // (nickname if the user set one, else the model). Present only for
        // device-scoped sensors so the metric line above stays generic.
        Label {
            visible: root.tileDeviceLabel.length > 0
            text: root.tileDeviceLabel
            color: app.tokens.textPrimary
            opacity: 0.55
            font.pixelSize: Kirigami.Theme.smallFont.pixelSize
            elide: Text.ElideRight
            Layout.fillWidth: true
            Accessible.ignored: true
        }

        Item { Layout.fillHeight: true; Layout.fillWidth: true }

        // Body: switch between scalar, counter, state, and table renderers
        Loader {
            id: bodyLoader
            Layout.fillWidth: true
            Layout.fillHeight: true
            sourceComponent: {
                switch (root.tileKind) {
                    case "table": return tableRenderer;
                    case "state": return stateRenderer;
                    default: return scalarRenderer;
                }
            }
        }

        // C1 — Sparkline mini chart (scalar sensors only). Only drawn when
        // the series actually varies — a constant value (e.g. GPU memory
        // total) carries no trend and shouldn't get a chart.
        Canvas {
            id: sparklineCanvas
            Layout.fillWidth: true
            height: 36
            visible: root.tileKind !== "table" && root.__sparklineVaries

            onPaint: {
                const ctx = getContext("2d")
                const w = width
                const h = height
                ctx.clearRect(0, 0, w, h)
                const pts = root.tileSparkline
                if (!pts || pts.length < 2) return
                let min = pts[0], max = pts[0]
                for (let k = 1; k < pts.length; ++k) {
                    if (pts[k] < min) min = pts[k]
                    if (pts[k] > max) max = pts[k]
                }
                const range = Math.max(max - min, 1e-10)
                ctx.strokeStyle = app.tokens.accent || "#4fc3f7"
                ctx.lineWidth = 1.5
                ctx.beginPath()
                for (let i = 0; i < pts.length; ++i) {
                    const x = (i / (pts.length - 1)) * w
                    const y = h - ((pts[i] - min) / range) * (h - 6) - 3
                    i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)
                }
                ctx.stroke()
            }

            // Repaint when sparkline data changes
            Connections {
                target: root
                function onTileSparklineChanged() { sparklineCanvas.requestPaint() }
            }
        }

        Item { Layout.fillHeight: true; Layout.fillWidth: true }
    }

    // --- Scalar (default) ---
    Component {
        id: scalarRenderer
        Label {
            text: root.tileValue
            color: app.tokens.textPrimary
            font.pixelSize: Kirigami.Theme.defaultFont.pixelSize * 3
            font.weight: Font.Medium
            Layout.alignment: Qt.AlignCenter
            Accessible.ignored: true
        }
    }

    // --- State ---
    Component {
        id: stateRenderer
        Item {
            id: stateRoot

            readonly property color statusColor: {
                var s = root.tileValue.toLowerCase();
                if (s === "up" || s === "running" || s === "active") return app.tokens.positive;
                if (s === "down" || s === "error" || s === "dead") return app.tokens.negative;
                return app.tokens.neutral;
            }

            RowLayout {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: app.tokens.spaceS

                Rectangle {
                    Layout.alignment: Qt.AlignVCenter
                    implicitWidth: 8
                    implicitHeight: 8
                    radius: 2
                    color: stateRoot.statusColor
                }

                Label {
                    text: root.tileValue
                    color: app.tokens.textPrimary
                    opacity: 0.92
                    font.pixelSize: app.tokens.textDisplay
                    font.weight: app.tokens.weightMedium
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    Accessible.ignored: true
                }
            }
        }
    }

    // --- Table ---
    Component {
        id: tableRenderer
        ScrollView {
            id: tableView
            clip: true
            ScrollBar.horizontal.policy: ScrollBar.AlwaysOff
            ListView {
                id: tableList
                model: root.tileRows
                interactive: true
                boundsBehavior: Flickable.StopAtBounds
                delegate: RowLayout {
                    width: tableView.availableWidth
                    spacing: Kirigami.Units.smallSpacing

                    Repeater {
                        model: modelData
                        delegate: Label {
                            text: {
                                if (typeof modelData === 'object' && modelData !== null) {
                                    if (modelData.text !== undefined) return modelData.text;
                                    if (modelData.number !== undefined) return Number(modelData.number).toFixed(1);
                                    if (modelData.bytes !== undefined) return formatBytes(modelData.bytes);
                                    return "";
                                }
                                return String(modelData);
                            }
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                            color: app.tokens.textPrimary
                            font.pixelSize: Kirigami.Theme.defaultFont.pixelSize * 0.85
                        }
                    }
                }
            }
        }
    }

    function formatBytes(b) {
        if (b >= 1099511627776) return (b / 1099511627776).toFixed(2) + " TiB";
        if (b >= 1073741824) return (b / 1073741824).toFixed(2) + " GiB";
        if (b >= 1048576) return (b / 1048576).toFixed(2) + " MiB";
        if (b >= 1024) return (b / 1024).toFixed(2) + " KiB";
        return b + " B";
    }
}
