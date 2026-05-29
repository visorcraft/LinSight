// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// About page — brand hero, feature highlights, links to license +
// credits. Mirrors Grexa's About structure adapted for LinSight's
// scope (system monitor vs file search).

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.ScrollablePage {
    id: page
    padding: 0
    titleDelegate: Item {}
    globalToolBarStyle: Kirigami.ApplicationHeaderStyle.None

    signal navigateRequested(string pageKey)

    Kirigami.Theme.inherit: false
    Kirigami.Theme.colorSet: Kirigami.Theme.View
    Kirigami.Theme.backgroundColor: app.tokens.surface0
    Kirigami.Theme.textColor: app.tokens.textPrimary

    readonly property var features: [
        { icon: "video-display-symbolic", title: qsTr("Multi-GPU"),
          body: qsTr("NVIDIA NVML, Intel xe driver iGPUs and Battlemage/Arc discrete cards on one Overview page.") },
        { icon: "preferences-other-symbolic", title: qsTr("Runtime plugins"),
          body: qsTr("Drop a `.so` into ~/.local/share/linsight/plugins/ for hardware that isn't built in.") },
        { icon: "view-statistics-symbolic", title: qsTr("Prometheus + SQLite"),
          body: qsTr("Opt-in always-on mode exposes /metrics and records history without the GUI open.") },
        { icon: "network-server-symbolic", title: qsTr("SSH remote"),
          body: qsTr("`linsight --connect ssh://user@host` attaches a local window to a remote linsightd.") }
    ]

    // The "Visit", "Credits", and "Licenses" buttons below use the
    // shared ThemedButton component (qml/ThemedButton.qml) — the same
    // flat, accent-tinted, theme-aware button used on the Settings,
    // Alerts, and Dashboard headers.

    ColumnLayout {
        width: page.width
        spacing: 0

        // -- Page header strip ------------------------------------
        // Matches Grexa's About layout: a 76px title strip ("About"
        // + one-line caption), then the substantive content lives in
        // cards beneath. The wordmark moves into the hero card below
        // rather than living naked at this height.
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: app.tokens.pageHeaderHeight
            color: app.tokens.surface0
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: 1
                color: app.tokens.separator
            }
            ColumnLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceXL
                anchors.rightMargin: app.tokens.spaceXL
                spacing: 1
                Layout.alignment: Qt.AlignVCenter
                Controls.Label {
                    text: qsTr("About")
                    font.pixelSize: app.tokens.textHeading
                    font.weight: app.tokens.weightBold
                    font.family: app.tokens.sansFamily
                }
                Controls.Label {
                    text: qsTr("Built on Rust + Qt 6 / Kirigami via cxx-qt.")
                    font.pixelSize: app.tokens.textCaption + 1
                    font.family: app.tokens.sansFamily
                    opacity: 0.6
                }
            }
        }

        // -- Brand hero card --------------------------------------
        // 168px card with an accent-mute gradient halo on the left,
        // the LinSight icon, the wordmark + tagline, and a pill row
        // with version + license. Same anatomy Grexa uses; brand
        // image swapped to LinSight's packaged 128px PNG.
        Item {
            Layout.fillWidth: true
            Layout.leftMargin: app.tokens.spaceXL
            Layout.rightMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceXL
            Layout.preferredHeight: 168

            Rectangle {
                anchors.fill: parent
                radius: app.tokens.radiusCard
                color: app.tokens.surface1
                border.color: app.tokens.separator
                border.width: 1

                Rectangle {
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: 240
                    radius: parent.radius
                    gradient: Gradient {
                        orientation: Gradient.Horizontal
                        GradientStop { position: 0.0; color: app.tokens.accentMute }
                        GradientStop { position: 1.0; color: "transparent" }
                    }
                }

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: app.tokens.spaceXL
                    anchors.rightMargin: app.tokens.spaceXL
                    spacing: app.tokens.spaceXL

                    Image {
                        source: "qrc:/qt/qml/com/visorcraft/LinSight/resources/linsight-128.png"
                        sourceSize.width: 224
                        sourceSize.height: 224
                        Layout.preferredWidth: 112
                        Layout.preferredHeight: 112
                        smooth: true
                        mipmap: true
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: app.tokens.spaceXS
                        Controls.Label {
                            text: "LinSight"
                            font.pixelSize: app.tokens.textDisplay
                            font.weight: app.tokens.weightBold
                            font.family: app.tokens.sansFamily
                        }
                        Controls.Label {
                            text: qsTr("A Linux system-monitoring dashboard with multi-GPU support and runtime plugins.")
                            font.pixelSize: app.tokens.textBody + 1
                            font.family: app.tokens.sansFamily
                            opacity: 0.7
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                        RowLayout {
                            spacing: app.tokens.spaceS
                            Layout.topMargin: app.tokens.spaceS
                            Rectangle {
                                radius: app.tokens.radiusPill
                                color: app.tokens.accentMute
                                border.color: app.tokens.accent
                                border.width: 1
                                implicitHeight: 26
                                implicitWidth: versionLabel.implicitWidth + app.tokens.spaceL * 2
                                Controls.Label {
                                    id: versionLabel
                                    anchors.centerIn: parent
                                    text: qsTr("v%1").arg(Qt.application.version)
                                    font.pixelSize: app.tokens.textCaption + 1
                                    font.weight: app.tokens.weightSemibold
                                    font.family: app.tokens.monoFamily
                                    color: app.tokens.accent
                                }
                            }
                            Rectangle {
                                radius: app.tokens.radiusPill
                                color: "transparent"
                                border.color: app.tokens.separator
                                border.width: 1
                                implicitHeight: 26
                                implicitWidth: licenseLabel.implicitWidth + app.tokens.spaceL * 2
                                Controls.Label {
                                    id: licenseLabel
                                    anchors.centerIn: parent
                                    text: qsTr("GPL v3")
                                    font.pixelSize: app.tokens.textCaption
                                    opacity: 0.78
                                    font.family: app.tokens.sansFamily
                                }
                            }
                            Rectangle {
                                radius: app.tokens.radiusPill
                                color: "transparent"
                                border.color: app.tokens.separator
                                border.width: 1
                                implicitHeight: 26
                                implicitWidth: platformLabel.implicitWidth + app.tokens.spaceL * 2
                                Controls.Label {
                                    id: platformLabel
                                    anchors.centerIn: parent
                                    text: qsTr("Linux · Qt 6")
                                    font.pixelSize: app.tokens.textCaption
                                    opacity: 0.78
                                    font.family: app.tokens.monoFamily
                                }
                            }
                        }
                    }
                }
            }
        }

        // -- Feature pills ---------------------------------------
        GridLayout {
            Layout.fillWidth: true
            Layout.leftMargin: app.tokens.spaceXL
            Layout.rightMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceXL
            columns: 2
            rowSpacing: app.tokens.spaceM
            columnSpacing: app.tokens.spaceM
            Repeater {
                model: page.features
                delegate: Rectangle {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 92
                    color: app.tokens.surface1
                    radius: app.tokens.radiusCard
                    border.color: app.tokens.separator
                    border.width: 1
                    RowLayout {
                        anchors.fill: parent
                        anchors.margins: app.tokens.spaceL
                        spacing: app.tokens.spaceM
                        Kirigami.Icon {
                            source: modelData.icon
                            implicitWidth: 28
                            implicitHeight: 28
                            color: app.tokens.accent
                            opacity: 0.9
                            isMask: true
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 1
                            Controls.Label {
                                text: modelData.title
                                font.weight: app.tokens.weightSemibold
                                font.pixelSize: app.tokens.textBodyEmphasis
                            }
                            Controls.Label {
                                text: modelData.body
                                wrapMode: Text.WordWrap
                                Layout.fillWidth: true
                                opacity: 0.78
                                font.pixelSize: app.tokens.textCaption + 1
                            }
                        }
                    }
                }
            }
        }

        // -- "Built for" callout ----------------------------------
        // Mirrors LinSync's About card: the app icon, a one-line pitch
        // + supporting sentence, and a "Visit" button (styled like the
        // Credits / Licenses buttons below) that opens the GitHub repo.
        // Replaces the old bare repo-link text — the button is now the
        // canonical repo link.
        Rectangle {
            Layout.fillWidth: true
            Layout.leftMargin: app.tokens.spaceXL
            Layout.rightMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceXL
            Layout.preferredHeight: 96
            radius: app.tokens.radiusCard
            color: app.tokens.surface1
            border.color: app.tokens.separator
            border.width: 1

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: app.tokens.spaceL
                anchors.rightMargin: app.tokens.spaceL
                spacing: app.tokens.spaceM

                Image {
                    Layout.preferredWidth: 56
                    Layout.preferredHeight: 56
                    Layout.alignment: Qt.AlignVCenter
                    source: "qrc:/qt/qml/com/visorcraft/LinSight/resources/linsight-128.png"
                    sourceSize.width: 112
                    sourceSize.height: 112
                    fillMode: Image.PreserveAspectFit
                    smooth: true
                    mipmap: true
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignVCenter
                    spacing: 2
                    Controls.Label {
                        text: qsTr("LinSight is built for real-time system monitoring.")
                        font.pixelSize: app.tokens.textBodyEmphasis
                        font.weight: app.tokens.weightBold
                        font.family: app.tokens.sansFamily
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        text: qsTr("Watch CPU, memory, GPU, NVMe, and network sensors live through a Rust daemon with a Qt/Kirigami shell.")
                        font.pixelSize: app.tokens.textCaption + 1
                        font.family: app.tokens.sansFamily
                        opacity: 0.78
                    }
                }

                ThemedButton {
                    Layout.alignment: Qt.AlignVCenter
                    icon.name: "go-next-symbolic"
                    text: qsTr("Visit LinSight")
                    onClicked: Qt.openUrlExternally("https://github.com/visorcraft/linsight")
                }
            }
        }

        // -- Licenses & credits -----------------------------------
        // Reuses LinSight's SettingsCard (title + subtitle + slot) so
        // the box matches LinSync's About card: a heading, a one-line
        // pointer to the bundled third-party notices, and Credits /
        // Licenses buttons that still route through navigateRequested.
        SettingsCard {
            Layout.fillWidth: true
            Layout.leftMargin: app.tokens.spaceXL
            Layout.rightMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceL
            title: qsTr("Licenses & Credits")
            subtitle: qsTr("Every direct + transitive crate, with version and license expression, is documented in docs/credits-third-party.md.")

            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceM

                ThemedButton {
                    icon.name: "view-list-details-symbolic"
                    text: qsTr("Credits")
                    onClicked: page.navigateRequested("credits")
                }

                ThemedButton {
                    icon.name: "view-list-text-symbolic"
                    text: qsTr("Licenses")
                    onClicked: page.navigateRequested("licenses")
                }

                Item { Layout.fillWidth: true }
            }
        }

        // -- Footer -----------------------------------------------
        // Centered attribution line, mirroring LinSync's About.
        Controls.Label {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: app.tokens.spaceL
            Layout.bottomMargin: app.tokens.spaceXL
            textFormat: Text.RichText
            text: qsTr("Built by <b>VisorCraft</b>") + "  ·  " + qsTr("Powered by Rust, Qt 6, and Kirigami")
            font.pixelSize: app.tokens.textCaption
            font.family: app.tokens.sansFamily
            color: app.tokens.textPrimary
            opacity: 0.55
        }
    }
}
