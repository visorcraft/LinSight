// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Theme-aware dropdown that matches ThemedButton: a surface2 field with
// a separator border that lights to the active accent on hover / focus,
// a hand-drawn chevron (so it never depends on the icon theme shipping
// a down-arrow), and a themed popup whose highlighted row uses the same
// accent-tint wash as the buttons. Colors come from DesignTokens, so a
// theme switch restyles every instance. Instances set `model`,
// `textRole`, `valueRole`, `currentIndex`, and `onActivated` exactly
// like a plain Controls.ComboBox.

import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Controls.ComboBox {
    id: control
    hoverEnabled: true
    implicitHeight: 36

    contentItem: Controls.Label {
        leftPadding: app.tokens.spaceM
        rightPadding: control.indicator.width + app.tokens.spaceM
        text: control.displayText
        color: app.tokens.textPrimary
        opacity: control.enabled ? 1.0 : 0.5
        font.family: app.tokens.sansFamily
        font.pixelSize: app.tokens.textBody
        verticalAlignment: Text.AlignVCenter
        horizontalAlignment: Text.AlignLeft
        elide: Text.ElideRight
    }

    // Hand-drawn chevron — repaints on a theme switch via the
    // textPrimary NOTIFY so its color tracks the active palette.
    indicator: Canvas {
        id: chevron
        x: control.width - width - app.tokens.spaceM
        y: (control.height - height) / 2
        width: 10
        height: 6
        opacity: control.enabled ? 0.7 : 0.35
        onPaint: {
            const ctx = getContext("2d")
            ctx.reset()
            ctx.strokeStyle = app.tokens.textPrimary
            ctx.lineWidth = 1.5
            ctx.lineJoin = "round"
            ctx.lineCap = "round"
            ctx.beginPath()
            ctx.moveTo(0.75, 0.75)
            ctx.lineTo(width / 2, height - 0.75)
            ctx.lineTo(width - 0.75, 0.75)
            ctx.stroke()
        }
        Connections {
            target: app.tokens
            function onTextPrimaryChanged() { chevron.requestPaint() }
        }
    }

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

    // The popup ListView is driven by the RAW `control.model` with its
    // delegate defined inline here — NOT by `control.delegateModel`. A
    // DelegateModel can only incubate each delegate in one place at a
    // time, and sharing the ComboBox's delegateModel with a custom popup
    // ListView left whole rows blank (the "blank spot above GPUs"): the
    // item at currentIndex, or the rest of the list, depending on how the
    // model was gated. Driving the ListView from the raw model gives it
    // its own delegate instances, and we wire selection + the `activated`
    // signal by hand in onClicked so the page-level `onActivated:`
    // handlers (applyTheme / applyStartPage / applySampleIntervalMs)
    // still fire.
    popup: Controls.Popup {
        y: control.height + 2
        width: control.width
        implicitHeight: Math.min(contentItem.implicitHeight + 2, 320)
        padding: 1

        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: control.model
            currentIndex: control.currentIndex
            Controls.ScrollIndicator.vertical: Controls.ScrollIndicator {}

            delegate: Controls.ItemDelegate {
                id: itemDelegate
                width: ListView.view ? ListView.view.width : control.width
                hoverEnabled: true
                contentItem: Controls.Label {
                    // Resolve the row's display text for both model kinds:
                    // a JS array exposes the row object as `modelData`; a
                    // ListModel exposes named roles on `model`. Try
                    // `modelData` first, fall back to `model`, then
                    // coalesce to "" so Qt never has to assign `undefined`
                    // to the label's QString text.
                    text: {
                        if (!control.textRole)
                            return (modelData === undefined || modelData === null) ? "" : modelData
                        var v
                        if (modelData !== undefined && modelData !== null
                                && typeof modelData === "object")
                            v = modelData[control.textRole]
                        if ((v === undefined || v === null) && model !== undefined && model !== null)
                            v = model[control.textRole]
                        return (v === undefined || v === null) ? "" : v
                    }
                    color: app.tokens.textPrimary
                    font.family: app.tokens.sansFamily
                    font.pixelSize: app.tokens.textBody
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
                background: Rectangle {
                    color: itemDelegate.hovered
                        ? Kirigami.ColorUtils.tintWithAlpha(app.tokens.surface2, app.tokens.accent, 0.18)
                        : (control.currentIndex === index
                            ? Kirigami.ColorUtils.tintWithAlpha(app.tokens.surface2, app.tokens.accent, 0.10)
                            : "transparent")
                }
                onClicked: {
                    control.currentIndex = index
                    control.activated(index)
                    control.popup.close()
                }
            }
        }

        background: Rectangle {
            color: app.tokens.surface1
            border.color: app.tokens.separator
            border.width: 1
            radius: app.tokens.radiusInput
        }
    }
}
