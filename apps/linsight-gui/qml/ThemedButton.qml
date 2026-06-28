// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Flat, theme-aware action button. The hover / press wash is tinted
// toward the active theme's accent (ColorUtils.tintWithAlpha) so it
// reads on both light and dark surfaces — a plain Qt.darker() produced
// no visible change on dark themes. `accent` / `surface2` / `separator`
// come from DesignTokens, which tracks the selected theme, so every
// instance restyles automatically on a theme switch.
//
// Originally the inline `AboutButton` in AboutPage.qml; promoted to a
// shared component so the Settings / Alerts / Dashboard headers share
// one source of truth. Instances set `text`, `icon.name`, and
// `onClicked` exactly like a plain Controls.Button.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Controls.Button {
    id: control
    hoverEnabled: true

    contentItem: RowLayout {
        spacing: 6
        Kirigami.Icon {
            visible: control.icon.name.length > 0
            source: control.icon.name
            color: app.tokens.textPrimary
            isMask: true
            implicitWidth: 16
            implicitHeight: 16
        }
        Controls.Label {
            text: control.text
            color: app.tokens.textPrimary
            font.family: app.tokens.sansFamily
        }
    }

    background: Rectangle {
        radius: app.tokens.radiusButton
        border.width: 1
        color: control.down
            ? Kirigami.ColorUtils.tintWithAlpha(app.tokens.surface2, app.tokens.accent, 0.30)
            : control.hovered
                ? Kirigami.ColorUtils.tintWithAlpha(app.tokens.surface2, app.tokens.accent, 0.16)
                : app.tokens.surface2
        border.color: (control.hovered || control.down) ? app.tokens.accent : app.tokens.separator
        Behavior on color { ColorAnimation { duration: app.tokens.durationSnap } }
        Behavior on border.color { ColorAnimation { duration: app.tokens.durationSnap } }
    }
}
