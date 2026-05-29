// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Sidebar navigation row. 36px tall, icon + label, the active row
// paints a soft accent-tinted pill across its width.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Item {
    id: root
    height: app.tokens.navRowHeight

    property string label: ""
    property string iconName: ""
    property bool active: false
    property bool compact: false
    signal triggered()

    // Make the row focusable + keyboard-actuable. Without these, NavItem
    // is mouse-only: Tab focus skips it, Enter/Space do nothing, and
    // screen readers see a bare Item with no semantic role.
    focus: false
    activeFocusOnTab: true
    Accessible.role: Accessible.MenuItem
    Accessible.name: root.label
    Accessible.checkable: false
    Keys.onReturnPressed: root.triggered()
    Keys.onEnterPressed: root.triggered()
    Keys.onSpacePressed: root.triggered()

    // Active-state pill: a soft accent-tinted wash behind the current
    // row, plus the focus/active border. Animated so it eases in and out
    // as you navigate between pages.
    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: app.tokens.spaceS
        anchors.rightMargin: app.tokens.spaceS
        anchors.topMargin: 1
        anchors.bottomMargin: 1
        radius: app.tokens.radiusInput
        color: root.active ? app.tokens.accentMute : "transparent"
        Behavior on color { ColorAnimation { duration: app.tokens.durationSnap } }
        border.color: {
            if (root.activeFocus) return app.tokens.accent
            if (root.active) {
                return Qt.rgba(app.tokens.accent.r, app.tokens.accent.g, app.tokens.accent.b, 0.25)
            }
            return "transparent"
        }
        border.width: root.activeFocus ? 2 : 1
    }

    // Hover / press highlight. Deliberately NOT animated: the old shared
    // `Behavior on color` faded this out over durationSnap, so moving the
    // mouse between rows briefly highlighted BOTH the row you left (still
    // fading) and the row you entered. Painting it instantly — and only
    // for the row actually under the cursor — guarantees exactly one row
    // is ever highlighted. Suppressed while active so it doesn't muddy the
    // accent pill above.
    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: app.tokens.spaceS
        anchors.rightMargin: app.tokens.spaceS
        anchors.topMargin: 1
        anchors.bottomMargin: 1
        radius: app.tokens.radiusInput
        visible: !root.active && (mouseArea.containsMouse || mouseArea.containsPress)
        color: mouseArea.containsPress ? app.tokens.surface2 : app.tokens.surface1
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: app.tokens.spaceL
        anchors.rightMargin: app.tokens.spaceL
        spacing: app.tokens.spaceM

        Kirigami.Icon {
            source: root.iconName
            implicitWidth: 18
            implicitHeight: 18
            Layout.alignment: root.compact ? Qt.AlignHCenter : Qt.AlignLeft
            Layout.fillWidth: root.compact
            color: root.active ? app.tokens.accent : app.tokens.textPrimary
            opacity: root.active ? 1.0 : 0.75
            isMask: true
            Behavior on color { ColorAnimation { duration: app.tokens.durationSnap } }
        }
        Controls.Label {
            text: root.label
            font.pixelSize: app.tokens.textBody
            font.family: app.tokens.sansFamily
            font.weight: root.active ? app.tokens.weightSemibold : app.tokens.weightNormal
            color: root.active ? app.tokens.accent : app.tokens.textPrimary
            opacity: root.active ? 1.0 : 0.88
            Layout.fillWidth: true
            visible: !root.compact
            Behavior on color { ColorAnimation { duration: app.tokens.durationSnap } }
        }
    }

    Controls.ToolTip.visible: root.compact && mouseArea.containsMouse
    Controls.ToolTip.text: root.label
    Controls.ToolTip.delay: 400

    MouseArea {
        id: mouseArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.triggered()
    }
}
