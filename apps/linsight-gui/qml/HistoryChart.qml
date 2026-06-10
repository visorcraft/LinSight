// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Canvas line chart for a single sensor's history.
//
// Props:
//   samples     — array of {t: micros, v: number} points (parsed by parent)
//   accentColor — stroke colour; falls back to app.tokens.accent
//   unitLabel   — short suffix displayed in stat pills (e.g. "°C", "%", "MHz")
//   mini        — when true: no stat pills, no labels, thinner stroke (1.5 px),
//                 no fill, zero internal padding — sparkline-strip mode.

import QtQuick
import QtQuick.Layouts

Item {
    id: root

    // Full-form input: array of {t: micros, v: number} points.
    property var samples: []
    // Fast-path input: plain f64 array (e.g. from tilesJson sparkline).
    // When non-empty this takes precedence over `samples`; each value is
    // mapped to {t: index, v: value} internally. Only one of the two
    // inputs should be set at a time.
    property var values: []
    property color accentColor: app.tokens.accent
    property string unitLabel: ""
    // Compact sparkline-strip rendering mode. Hides stat pills and fill;
    // uses a thinner stroke and zero internal padding.
    property bool mini: false

    // Internal: resolved point array (either samples or values-mapped).
    readonly property var resolvedPts: {
        if (Array.isArray(root.values) && root.values.length > 0) {
            const v = root.values
            const out = new Array(v.length)
            for (let i = 0; i < v.length; ++i) out[i] = { t: i, v: v[i] }
            return out
        }
        return root.samples
    }

    // ---- derived stats (recomputed in one pass when the resolved series changes) ---
    property real statMin: 0
    property real statMax: 0
    property real statAvg: 0
    property bool hasData: false

    // resolvedPts re-evaluates whenever samples or values changes, so a
    // single handler here covers both inputs.
    onResolvedPtsChanged: root._computeStats()
    onAccentColorChanged: chart.requestPaint()

    // Format a raw value with the appropriate scaled unit suffix.
    // Bytes → KiB/MiB/GiB/TiB; Hz → kHz/MHz/GHz; % and others → toFixed(1)+unit.
    function formatValue(v, unit) {
        if (unit === "B" || unit === "B/s") {
            const suffix = unit === "B/s" ? "/s" : ""
            const abs = Math.abs(v)
            if (abs >= 1099511627776) return (v / 1099511627776).toFixed(2) + " TiB" + suffix
            if (abs >= 1073741824)    return (v / 1073741824).toFixed(2)    + " GiB" + suffix
            if (abs >= 1048576)       return (v / 1048576).toFixed(2)       + " MiB" + suffix
            if (abs >= 1024)          return (v / 1024).toFixed(2)          + " KiB" + suffix
            return v.toFixed(0) + " B" + suffix
        }
        if (unit === "Hz") {
            const abs = Math.abs(v)
            if (abs >= 1e9) return (v / 1e9).toFixed(2) + " GHz"
            if (abs >= 1e6) return (v / 1e6).toFixed(0) + " MHz"
            if (abs >= 1e3) return (v / 1e3).toFixed(0) + " kHz"
            return v.toFixed(0) + " Hz"
        }
        return Number(v).toFixed(1) + (unit ? " " + unit : "")
    }

    function _computeStats() {
        const pts = root.resolvedPts
        if (!Array.isArray(pts) || pts.length === 0) {
            root.hasData = false
            root.statMin = 0; root.statMax = 0; root.statAvg = 0
            chart.requestPaint()
            return
        }
        root.hasData = true
        let mn = pts[0].v, mx = pts[0].v, sum = 0
        for (let i = 0; i < pts.length; ++i) {
            const v = pts[i].v
            if (v < mn) mn = v
            if (v > mx) mx = v
            sum += v
        }
        root.statMin = mn
        root.statMax = mx
        root.statAvg = sum / pts.length
        chart.requestPaint()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: root.mini ? 0 : app.tokens.spaceS

        // ---- chart canvas ---------------------------------------------------
        Canvas {
            id: chart
            Layout.fillWidth: true
            Layout.fillHeight: true

            onWidthChanged:  requestPaint()
            onHeightChanged: requestPaint()

            onPaint: {
                const ctx = getContext("2d")
                const w = width
                const h = height
                ctx.clearRect(0, 0, w, h)

                const pts = root.resolvedPts
                if (!pts || pts.length < 2) {
                    // Single point or empty — draw a horizontal centre line.
                    if (pts && pts.length === 1) {
                        ctx.strokeStyle = root.accentColor
                        ctx.lineWidth = 1.5
                        ctx.beginPath()
                        ctx.moveTo(0, h / 2)
                        ctx.lineTo(w, h / 2)
                        ctx.stroke()
                    }
                    return
                }

                // Y-axis: pad by 8% of range so the line isn't glued to the edge.
                const mn = root.statMin
                const mx = root.statMax
                const range = Math.max(mx - mn, Math.abs(mx) * 1e-9, 1e-10)
                const pad = range * 0.08
                const yMin = mn - pad
                const yMax = mx + pad
                const yRange = yMax - yMin

                function mapY(v) {
                    return h - ((v - yMin) / yRange) * (h - 1)
                }

                if (!root.mini) {
                    // Subtle fill under the line (full chart only).
                    const fillColor = Qt.rgba(root.accentColor.r,
                                              root.accentColor.g,
                                              root.accentColor.b, 0.12)
                    ctx.fillStyle = fillColor
                    ctx.beginPath()
                    for (let i = 0; i < pts.length; ++i) {
                        const x = (i / (pts.length - 1)) * w
                        const y = mapY(pts[i].v)
                        i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)
                    }
                    ctx.lineTo(w, h)
                    ctx.lineTo(0, h)
                    ctx.closePath()
                    ctx.fill()
                }

                // Stroke.
                ctx.strokeStyle = root.accentColor
                ctx.lineWidth = 1.5
                ctx.lineJoin = "round"
                ctx.beginPath()
                for (let i = 0; i < pts.length; ++i) {
                    const x = (i / (pts.length - 1)) * w
                    const y = mapY(pts[i].v)
                    i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)
                }
                ctx.stroke()
            }
        }

        // ---- stat pills (hidden in mini/sparkline mode) ----------------------
        Row {
            Layout.alignment: Qt.AlignHCenter
            spacing: app.tokens.spaceL
            visible: root.hasData && !root.mini

            Repeater {
                model: [
                    { label: qsTr("Min"),  value: root.statMin },
                    { label: qsTr("Avg"),  value: root.statAvg },
                    { label: qsTr("Max"),  value: root.statMax },
                ]
                delegate: Column {
                    required property var modelData
                    spacing: 0
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: modelData.label
                        font.pixelSize: app.tokens.textCaption - 1
                        font.family: app.tokens.sansFamily
                        opacity: 0.55
                        color: app.tokens.textPrimary
                    }
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: root.formatValue(modelData.value, root.unitLabel)
                        font.pixelSize: app.tokens.textBody
                        font.family: app.tokens.monoFamily
                        color: app.tokens.textPrimary
                    }
                }
            }
        }
    }
}
