// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// LinSight design tokens — single source of truth for spacing,
// type sizes, radii, and color accents. Instantiated once in
// Main.qml as `id: tokens`; pages reach the values via
// `app.tokens.spaceM`, `app.tokens.accent`, etc.
//
// Colors derive from Kirigami.Theme so light/dark/accent inherit
// from the host KDE Plasma color scheme automatically. The static
// accent uses a fixed cyan→indigo gradient since Kirigami doesn't
// expose a second accent slot we can lean on.

import QtQuick
import org.kde.kirigami as Kirigami

QtObject {
    // Spacing scale on a 4px rhythm.
    readonly property int spaceXS:   4
    readonly property int spaceS:    8
    readonly property int spaceM:   12
    readonly property int spaceL:   16
    readonly property int spaceXL:  24
    readonly property int spaceXXL: 32

    // Radii.
    readonly property int radiusButton: 6
    readonly property int radiusCard:   10
    readonly property int radiusInput:  8
    readonly property int radiusAvatar: 10
    readonly property int radiusPill:   999

    // Type scale.
    readonly property int textCaption:      11
    readonly property int textBody:         13
    readonly property int textBodyEmphasis: 14
    readonly property int textSubheading:   16
    readonly property int textHeading:      22
    readonly property int textDisplay:      30

    readonly property int weightNormal:   Font.Normal
    readonly property int weightMedium:   Font.Medium
    readonly property int weightSemibold: Font.DemiBold
    readonly property int weightBold:     Font.Bold

    readonly property int navRowHeight: 36

    // Page-header strip height. Used by every preset page (Overview,
    // Category, Settings, About, Licenses, Credits) and the canvas
    // editor. Previously copy-pasted as `height: 76` across each page.
    readonly property int pageHeaderHeight: 76

    // Motion: pass `--reduce-motion` (or the alias `--no-animations`)
    // on the command line to flatten every duration to 0 — the
    // snappy-but-bouncy sidebar collapse, the tile hover fade, and any
    // future tween become instant. Useful for vestibular-sensitive
    // users and for capturing flicker-free screenshots.
    //
    // Both flags are declared in `apps/linsight-gui/src/main.rs`'s
    // clap layer too, so they appear in `--help` and shell completion.
    // We still read `Qt.application.arguments` here because the
    // designtokens singleton is constructed before any Rust setter
    // could propagate. `indexOf` is sufficient for both names; the
    // previous `typeof Qt.application.arguments.includes === "function"`
    // guard was confused defensive coding.
    readonly property bool reduceMotion:
        Qt.application.arguments.indexOf("--reduce-motion") !== -1
        || Qt.application.arguments.indexOf("--no-animations") !== -1
    readonly property int durationSnap:  reduceMotion ? 0 : 120

    // Colors are routed through `app.preferences` so the active
    // theme can replace Kirigami.Theme entirely. For the `system`
    // theme the preferences model returns empty strings for surface
    // / text / separator roles — that's the fallback signal to use
    // the live Kirigami value. Named themes return concrete hex
    // strings that pin the look regardless of the Plasma color
    // scheme. Reading the `theme` qproperty inside `_pickColor()`
    // forces every binding in this object to re-evaluate when the
    // active theme changes, so the whole UI repaints on selection.
    readonly property string _activeTheme: app.preferences ? app.preferences.theme : ""

    function _pickColor(role, plasmaFallback) {
        if (!app.preferences) return plasmaFallback
        const _ = _activeTheme   // pull-binding dep so changes propagate
        const c = app.preferences.color(role)
        return (c && c.length > 0) ? c : plasmaFallback
    }

    readonly property color surface0:
        _pickColor("surface0", Kirigami.Theme.backgroundColor)
    readonly property color surface1:
        _pickColor("surface1", Qt.lighter(Kirigami.Theme.backgroundColor, 1.10))
    readonly property color surface2:
        _pickColor("surface2", Qt.lighter(Kirigami.Theme.backgroundColor, 1.22))
    readonly property color surfaceSidebar:
        _pickColor("surface_sidebar", Qt.darker(Kirigami.Theme.backgroundColor, 1.08))
    readonly property color textPrimary:
        _pickColor("text_primary", Kirigami.Theme.textColor)
    readonly property color textSecondary:
        _pickColor("text_secondary",
            Qt.rgba(Kirigami.Theme.textColor.r,
                    Kirigami.Theme.textColor.g,
                    Kirigami.Theme.textColor.b, 0.65))
    readonly property color separator: {
        const _ = _activeTheme
        const c = app.preferences ? app.preferences.color("separator_rgba") : ""
        return c.length > 0
            ? c
            : Qt.rgba(Kirigami.Theme.textColor.r,
                      Kirigami.Theme.textColor.g,
                      Kirigami.Theme.textColor.b, 0.10)
    }

    // Accent / accent_mute / accent_text are always specified — for
    // `system` it's the LinSight cyan-indigo; named themes ship their
    // own. Active nav rows use accent_mute for a soft wash.
    readonly property color accent:     _pickColor("accent",      "#6c8cff")
    readonly property color accentMute: _pickColor("accent_mute",
        Qt.rgba(0x6c/255, 0x8c/255, 0xff/255, 0.16))
    readonly property color accentText: _pickColor("accent_text", "white")

    // Pill surfaces follow LinSight's active theme, not the host
    // Kirigami background, so named light/dark themes keep contrast.
    readonly property color pillBackground: _pickColor("pill_background", surface2)

    // Status colors for badges and semantic UI elements.
    readonly property color positive: _pickColor("positive", Kirigami.Theme.positiveTextColor)
    readonly property color negative: _pickColor("negative", Kirigami.Theme.negativeTextColor)
    readonly property color neutral: _pickColor("neutral", Kirigami.Theme.neutralTextColor)

    // Monospace family probe — picks the first installed of a small
    // preferred list; falls back to the platform's generic monospace.
    readonly property string monoFamily: {
        const preferred = ["JetBrains Mono", "Fira Code", "Iosevka",
                           "Cascadia Mono", "Source Code Pro", "Hack",
                           "DejaVu Sans Mono"]
        const installed = Qt.fontFamilies()
        for (let i = 0; i < preferred.length; ++i) {
            if (installed.indexOf(preferred[i]) !== -1) return preferred[i]
        }
        return "monospace"
    }

    readonly property string sansFamily: {
        const preferred = ["Inter", "Inter Display", "Nunito Sans",
                           "IBM Plex Sans", "Source Sans 3",
                           "Source Sans Pro", "Noto Sans",
                           "Cantarell"]
        const installed = Qt.fontFamilies()
        for (let i = 0; i < preferred.length; ++i) {
            if (installed.indexOf(preferred[i]) !== -1) return preferred[i]
        }
        return ""
    }
}
