// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Canvas line chart for a single sensor's history.
//
// Props:
//   samples   — array of {t: micros, v: number} points (parsed by parent)
//   accentColor — stroke colour; falls back to app.tokens.accent
//   unitLabel   — short suffix displayed in stat pills (e.g. "°C", "%", "MHz")

import QtQuick
import QtQuick.Layouts

Item {
    id: root

    property var samples: []
    property color accentColor: app.tokens.accent
    property string unitLabel: ""

    // ---- derived stats (recomputed in one pass when samples change) ----------
    property real statMin: 0
    property real statMax: 0
    property real statAvg: 0
    property bool hasData: false

    onSamplesChanged: root._computeStats()

    function _computeStats() {
        const pts = root.samples
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
        spacing: app.tokens.spaceS

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

                const pts = root.samples
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
                const range = Math.max(mx - mn, 1e-10)
                const pad = range * 0.08
                const yMin = mn - pad
                const yMax = mx + pad
                const yRange = yMax - yMin

                function mapY(v) {
                    return h - ((v - yMin) / yRange) * (h - 1)
                }

                // Subtle fill under the line.
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

        // ---- stat pills -------------------------------------------------------
        Row {
            Layout.alignment: Qt.AlignHCenter
            spacing: app.tokens.spaceL
            visible: root.hasData

            Repeater {
                model: [
                    { label: qsTr("Min"),  value: root.statMin },
                    { label: qsTr("Avg"),  value: root.statAvg },
                    { label: qsTr("Max"),  value: root.statMax },
                ]
                delegate: Column {
                    required property var modelData
                    spacing: 1
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
                        text: Number(modelData.value).toFixed(1) + (root.unitLabel ? " " + root.unitLabel : "")
                        font.pixelSize: app.tokens.textBody
                        font.family: app.tokens.monoFamily
                        color: app.tokens.textPrimary
                    }
                }
            }
        }
    }
}
