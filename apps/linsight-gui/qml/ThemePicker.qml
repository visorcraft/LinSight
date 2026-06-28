// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Theme picker — a compact ComboBox dropdown. Originally a Flow grid
// of swatches; switched to a dropdown to match the convention the
// user wanted from Grexa. Each entry is `display_name`; an accent
// swatch sits next to the active item.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

ColumnLayout {
    id: root
    Layout.fillWidth: true
    spacing: app.tokens.spaceS

    readonly property var themes: app.preferences
        ? JSON.parse(app.preferences.themesJson())
        : []
    readonly property string activeId: app.preferences ? app.preferences.theme : ""

    function _indexOfActive() {
        for (let i = 0; i < root.themes.length; ++i) {
            if (root.themes[i].id === root.activeId) return i
        }
        return 0
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: app.tokens.spaceM

        // Accent swatch — visual reminder of which palette is active.
        Rectangle {
            implicitWidth: 18
            implicitHeight: 18
            radius: 9
            color: app.tokens.accent
            border.width: 1
            border.color: Qt.rgba(0, 0, 0, 0.25)
            Accessible.role: Accessible.Indicator
            Accessible.name: qsTr("Current theme accent")
        }

        ThemedComboBox {
            id: themeCombo
            Layout.fillWidth: true
            Layout.preferredHeight: 36
            model: root.themes
            textRole: "display_name"
            valueRole: "id"
            // `_indexOfActive` consults `themes` + `activeId` so the
            // initial selection mirrors what the preferences model
            // already loaded. The Component.onCompleted writeback
            // covers the edge where `themes` resolves after this
            // ComboBox finishes constructing.
            currentIndex: _indexOfActive()
            Component.onCompleted: currentIndex = root.activeId
                                       ? _indexOfActive() : 0
            onActivated: idx => {
                if (idx < 0 || idx >= root.themes.length) return
                const id = root.themes[idx].id
                if (id && app.preferences) app.preferences.applyTheme(id)
            }
            Accessible.role: Accessible.ComboBox
            Accessible.name: qsTr("Theme")
            Accessible.description: qsTr("Choose a built-in palette or follow the KDE Plasma color scheme.")
        }
    }

    Controls.Label {
        Layout.fillWidth: true
        text: {
            // Surface the help line in two forms so the picker is
            // useful even before the user opens the dropdown.
            const entry = root.themes[themeCombo.currentIndex] || {}
            return entry.is_system
                ? qsTr("Follows your KDE Plasma color scheme.")
                : qsTr("Built-in palette — overrides the Plasma scheme while LinSight is open.")
        }
        font.pixelSize: app.tokens.textCaption
        opacity: 0.6
        wrapMode: Text.WordWrap
    }
}
