// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// GPL-3.0 full text dialog. Loaded lazily from the bundled LICENSE
// resource the first time the dialog opens.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.Dialog {
    id: root
    title: qsTr("GNU General Public License v3")
    modal: true
    width: Math.min(app.width * 0.8, 880)
    height: Math.min(app.height * 0.85, 720)
    standardButtons: Controls.Dialog.Close

    // Loaded eagerly from the OverviewModel — Qt's QML
    // XMLHttpRequest on qrc:/ URLs doesn't reliably fire DONE on this
    // setup, so the model provides the bundled text directly.
    property QtObject dashModel: null
    property string gplText: dashModel ? dashModel.gplText() : ""

    contentItem: Controls.ScrollView {
        clip: true
        Controls.TextArea {
            readOnly: true
            wrapMode: Text.NoWrap
            selectByMouse: true
            font.family: app.tokens.monoFamily
            font.pixelSize: app.tokens.textCaption + 1
            text: root.gplText
        }
    }
}
