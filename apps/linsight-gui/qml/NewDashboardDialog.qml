// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.Dialog {
    id: dlg
    title: qsTr("New Dashboard")
    modal: true
    standardButtons: Controls.Dialog.Cancel | Controls.Dialog.Ok
    width: Math.min(420, parent ? parent.width - 40 : 420)

    // `dashboardCreated` fires with the new slug on success.
    // `dashboardFailed` fires with the `lastError` detail on failure.
    // The contract relies on `create` returning an empty QString for
    // failure rather than a `"error: ..."`-prefixed sentinel — the
    // banner-by-type rule from CLAUDE.md.
    signal dashboardCreated(string slug)
    signal dashboardFailed(string detail)

    onAboutToShow: {
        nameField.text = ""
        nameField.forceActiveFocus()
    }

    onAccepted: {
        const trimmed = nameField.text.replace(/^\s+|\s+$/g, "")
        if (trimmed.length === 0) return
        const slug = app.dashboards.create(trimmed).toString()
        if (slug.length > 0) {
            dlg.dashboardCreated(slug)
        } else {
            dlg.dashboardFailed(app.dashboards.lastError
                                || qsTr("Could not create dashboard."))
        }
    }

    contentItem: ColumnLayout {
        spacing: app.tokens.spaceM
        Controls.Label {
            text: qsTr("Name")
            font.pixelSize: app.tokens.textCaption
            opacity: 0.7
        }
        Controls.TextField {
            id: nameField
            Layout.fillWidth: true
            placeholderText: qsTr("e.g. Gaming Rig")
            // Submit on Enter when there's a non-empty name.
            Keys.onReturnPressed: if (text.length > 0) dlg.accept()
            Keys.onEnterPressed:  if (text.length > 0) dlg.accept()
        }
        Controls.Label {
            text: qsTr("Creates an empty dashboard. Drop sensors onto the canvas to populate it.")
            font.pixelSize: app.tokens.textCaption
            opacity: 0.55
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
    }
}
