// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

ColumnLayout {
    property string label: ""
    property real value: 0

    spacing: 0
    Controls.Label {
        text: parent.label
        font.pixelSize: app.tokens.textCaption
        opacity: 0.6
        color: app.tokens.textPrimary
    }
    Controls.Label {
        id: valueLabel
        text: ""
        font.pixelSize: app.tokens.textBody
        font.weight: app.tokens.weightSemibold
        color: app.tokens.textPrimary
    }

    function formatNetworkRate(bytesPerSec) {
        if (bytesPerSec >= 1024 * 1024 * 1024) return (bytesPerSec / (1024 * 1024 * 1024)).toFixed(2) + " GiB/s"
        if (bytesPerSec >= 1024 * 1024) return (bytesPerSec / (1024 * 1024)).toFixed(2) + " MiB/s"
        if (bytesPerSec >= 1024) return (bytesPerSec / 1024).toFixed(2) + " KiB/s"
        return bytesPerSec.toFixed(1) + " B/s"
    }

    onValueChanged: valueLabel.text = formatNetworkRate(value)
    Component.onCompleted: valueLabel.text = formatNetworkRate(value)
}
