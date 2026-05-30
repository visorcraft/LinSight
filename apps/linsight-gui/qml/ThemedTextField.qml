// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Theme-aware text field. Uses surface2 for background, with an accent-tinted
// border on hover/focus. Matches the style of ThemedComboBox and ThemedButton.
// Instances set `text`, `placeholderText`, `enabled`, `maximumLength`, etc.
// exactly like a plain Controls.TextField.

import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Controls.TextField {
    id: control
    hoverEnabled: true
    implicitHeight: 36

    leftPadding: app.tokens.spaceM
    rightPadding: app.tokens.spaceM

    font.family: app.tokens.sansFamily
    font.pixelSize: app.tokens.textBody
    color: app.tokens.textPrimary
    selectionColor: app.tokens.accent

    background: Rectangle {
        radius: app.tokens.radiusInput
        border.width: 1
        color: control.down
            ? Kirigami.ColorUtils.tintWithAlpha(app.tokens.surface2, app.tokens.accent, 0.30)
            : control.hovered
                ? Kirigami.ColorUtils.tintWithAlpha(app.tokens.surface2, app.tokens.accent, 0.16)
                : app.tokens.surface2
        border.color: (control.activeFocus || control.hovered || control.down)
            ? app.tokens.accent : app.tokens.separator
        Behavior on color { ColorAnimation { duration: app.tokens.durationSnap } }
        Behavior on border.color { ColorAnimation { duration: app.tokens.durationSnap } }
    }
}
