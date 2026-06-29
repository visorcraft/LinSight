// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

import "Shared.js" as Shared

ColumnLayout {
    id: root
    property string label: ""
    property real value: 0
    property bool isBytes: true

    spacing: 0
    Controls.Label {
        text: root.label
        font.pixelSize: app.tokens.textCaption
        opacity: 0.6
        color: app.tokens.textPrimary
    }
    Controls.Label {
        text: root.isBytes ? Shared.formatByteRate(root.value) : formatRate(root.value)
        font.pixelSize: app.tokens.textBody
        font.weight: app.tokens.weightSemibold
        color: app.tokens.textPrimary
    }

    function formatRate(rate) {
        if (rate >= 1000 * 1000 * 1000) return (rate / (1000 * 1000 * 1000)).toFixed(2) + " G/s"
        if (rate >= 1000 * 1000) return (rate / (1000 * 1000)).toFixed(2) + " M/s"
        if (rate >= 1000) return (rate / 1000).toFixed(2) + " K/s"
        return rate.toFixed(1) + " /s"
    }
}
