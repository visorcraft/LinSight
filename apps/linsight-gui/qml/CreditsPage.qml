// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Credits page — runtime components card + filterable Cargo crates
// table. Matches Grexa's CreditsPage anatomy; the runtime list is
// hand-curated (Qt, Kirigami, NVML, kernel drivers); the Cargo list
// is parsed at runtime from the cargo-about output bundled into
// the binary at build time.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    padding: 0
    titleDelegate: Item {}
    globalToolBarStyle: Kirigami.ApplicationHeaderStyle.None

    property QtObject dashModel: null
    property string filterText: ""
    property var crates: []

    readonly property int rowHeight: 36
    readonly property int nameColumnWidth: Math.max(210, Math.min(300, page.width * 0.25))
    readonly property int versionColumnWidth: 124
    readonly property int linkColumnWidth: 44

    // Runtime components — system libraries LinSight links / dlopens
    // at execution time. None are bundled into the GPL source
    // distribution; downstream packagers handle redistribution.
    readonly property var runtimeComponents: [
        {
            name: "Qt 6 (Core, Qml, Gui, Quick)",
            licenses: "LGPL-3.0 / GPL-3.0 / commercial",
            url: "https://www.qt.io"
        },
        {
            name: "KDE Frameworks 6 — Kirigami",
            licenses: "LGPL-2.1+",
            url: "https://invent.kde.org/frameworks/kirigami"
        },
        {
            name: "NVIDIA Management Library (libnvidia-ml)",
            licenses: "NVIDIA proprietary, redistributable",
            url: "https://developer.nvidia.com/nvidia-management-library-nvml"
        },
        {
            name: "Linux kernel sysfs / drm subsystem",
            licenses: "GPL-2.0",
            url: "https://kernel.org"
        },
        {
            name: "Linux kernel xe driver (Intel Arc)",
            licenses: "MIT / GPL-2.0 dual",
            url: "https://docs.kernel.org/gpu/xe/index.html"
        },
        {
            name: "libsensors (hwmon abstraction)",
            licenses: "LGPL-2.1+",
            url: "https://github.com/lm-sensors/lm-sensors"
        }
    ]

    readonly property var filteredCrates: {
        const needle = page.filterText.trim().toLowerCase()
        if (needle.length === 0) return page.crates
        return page.crates.filter(row =>
            String(row.name).toLowerCase().indexOf(needle) !== -1
                || String(row.version).toLowerCase().indexOf(needle) !== -1
                || String(row.license).toLowerCase().indexOf(needle) !== -1)
    }

    Kirigami.Theme.inherit: false
    Kirigami.Theme.colorSet: Kirigami.Theme.View
    Kirigami.Theme.backgroundColor: app.tokens.surface0
    Kirigami.Theme.textColor: app.tokens.textPrimary
    Kirigami.Theme.highlightColor: app.tokens.accent
    Kirigami.Theme.highlightedTextColor: app.tokens.accentText

    background: Rectangle { color: app.tokens.surface0 }

    Component.onCompleted: page.loadCredits()

    function loadCredits() {
        if (!page.dashModel) { page.crates = []; return }
        try {
            page.crates = JSON.parse(page.dashModel.thirdPartyCreditsJson())
        } catch (e) {
            page.crates = []
        }
    }

    function openUrl(url) {
        if (url && String(url).length > 0) Qt.openUrlExternally(url)
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // -- Page header strip ----------------------------------
        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 96
            color: app.tokens.surface1
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
                spacing: app.tokens.spaceXS
                Item { Layout.fillHeight: true }
                Controls.Label {
                    Layout.fillWidth: true
                    text: qsTr("Credits")
                    color: app.tokens.textPrimary
                    font.pixelSize: 26
                    font.weight: app.tokens.weightBold
                    font.family: app.tokens.sansFamily
                }
                Controls.Label {
                    Layout.fillWidth: true
                    text: qsTr("%1 Cargo crates · %2 runtime components")
                        .arg(page.crates.length)
                        .arg(page.runtimeComponents.length)
                    color: app.tokens.textPrimary
                    font.pixelSize: app.tokens.textCaption + 1
                    font.family: app.tokens.sansFamily
                    opacity: 0.62
                    elide: Text.ElideRight
                }
                Item { Layout.fillHeight: true }
            }
        }

        // -- Body -----------------------------------------------
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: app.tokens.spaceXL
            Layout.rightMargin: app.tokens.spaceXL
            Layout.topMargin: app.tokens.spaceL
            Layout.bottomMargin: app.tokens.spaceL
            spacing: app.tokens.spaceM

            // Runtime components card
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: runtimeContent.implicitHeight + app.tokens.spaceL * 2
                radius: app.tokens.radiusCard
                color: app.tokens.surface1
                border.color: app.tokens.separator
                border.width: 1
                ColumnLayout {
                    id: runtimeContent
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: app.tokens.spaceL
                    spacing: app.tokens.spaceS
                    Controls.Label {
                        Layout.fillWidth: true
                        text: qsTr("Runtime components")
                        color: app.tokens.textPrimary
                        font.pixelSize: app.tokens.textBodyEmphasis
                        font.weight: app.tokens.weightBold
                        font.family: app.tokens.sansFamily
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        text: qsTr("System libraries and kernel surfaces LinSight links against at execution. None are bundled — downstream packagers handle redistribution.")
                        color: app.tokens.textPrimary
                        font.pixelSize: app.tokens.textBody
                        font.family: app.tokens.sansFamily
                        opacity: 0.62
                        wrapMode: Text.WordWrap
                    }
                    Repeater {
                        model: page.runtimeComponents
                        delegate: RowLayout {
                            Layout.fillWidth: true
                            Layout.preferredHeight: 28
                            spacing: app.tokens.spaceM
                            Controls.Label {
                                Layout.preferredWidth: page.nameColumnWidth + 90
                                Layout.maximumWidth: page.nameColumnWidth + 220
                                text: modelData.name
                                color: app.tokens.textPrimary
                                font.pixelSize: app.tokens.textBody
                                font.weight: app.tokens.weightSemibold
                                font.family: app.tokens.sansFamily
                                elide: Text.ElideRight
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                text: modelData.licenses
                                color: app.tokens.textPrimary
                                font.pixelSize: app.tokens.textCaption + 1
                                font.family: app.tokens.monoFamily
                                opacity: 0.86
                                elide: Text.ElideRight
                            }
                            Controls.Button {
                                Layout.preferredWidth: 34
                                Layout.preferredHeight: 28
                                icon.name: "internet-services-symbolic"
                                display: Controls.AbstractButton.IconOnly
                                onClicked: page.openUrl(modelData.url)
                                Controls.ToolTip.text: qsTr("Open project website")
                                Controls.ToolTip.visible: hovered
                            }
                        }
                    }
                }
            }

            Controls.Label {
                Layout.fillWidth: true
                Layout.topMargin: app.tokens.spaceS
                text: qsTr("CARGO CRATES")
                color: app.tokens.textPrimary
                font.pixelSize: 10
                font.weight: app.tokens.weightSemibold
                font.family: app.tokens.sansFamily
                opacity: 0.5
            }

            // Filter + count
            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceM
                Controls.TextField {
                    id: filterField
                    Layout.fillWidth: true
                    placeholderText: qsTr("Filter by crate name or license…")
                    onTextChanged: page.filterText = text
                    Accessible.name: qsTr("Filter third-party credits")
                }
                Controls.Label {
                    Layout.preferredWidth: 68
                    text: qsTr("%1 / %2").arg(page.filteredCrates.length).arg(page.crates.length)
                    color: app.tokens.textPrimary
                    font.pixelSize: app.tokens.textCaption + 1
                    font.family: app.tokens.monoFamily
                    opacity: 0.62
                    horizontalAlignment: Text.AlignRight
                }
            }

            // Crates table
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.minimumHeight: 160
                radius: app.tokens.radiusCard
                color: app.tokens.surface1
                border.color: app.tokens.separator
                border.width: 1
                clip: true
                ColumnLayout {
                    anchors.fill: parent
                    spacing: 0
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 38
                        color: app.tokens.surface2
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: app.tokens.spaceL
                            anchors.rightMargin: app.tokens.spaceL
                            spacing: app.tokens.spaceM
                            Controls.Label {
                                Layout.preferredWidth: page.nameColumnWidth
                                text: qsTr("Crate")
                                color: app.tokens.textPrimary
                                font.pixelSize: app.tokens.textCaption + 1
                                font.weight: app.tokens.weightSemibold
                                opacity: 0.72
                            }
                            Controls.Label {
                                Layout.preferredWidth: page.versionColumnWidth
                                text: qsTr("Version")
                                color: app.tokens.textPrimary
                                font.pixelSize: app.tokens.textCaption + 1
                                font.weight: app.tokens.weightSemibold
                                opacity: 0.72
                            }
                            Controls.Label {
                                Layout.fillWidth: true
                                text: qsTr("License expression")
                                color: app.tokens.textPrimary
                                font.pixelSize: app.tokens.textCaption + 1
                                font.weight: app.tokens.weightSemibold
                                opacity: 0.72
                            }
                            Item { Layout.preferredWidth: page.linkColumnWidth }
                        }
                    }
                    ListView {
                        id: crateList
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        model: page.filteredCrates
                        delegate: Rectangle {
                            width: crateList.width
                            height: page.rowHeight
                            color: index % 2 === 1
                                ? Qt.rgba(app.tokens.surface2.r,
                                          app.tokens.surface2.g,
                                          app.tokens.surface2.b, 0.34)
                                : "transparent"
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: app.tokens.spaceL
                                anchors.rightMargin: app.tokens.spaceL
                                spacing: app.tokens.spaceM
                                Controls.Label {
                                    Layout.preferredWidth: page.nameColumnWidth
                                    text: modelData.name
                                    color: app.tokens.textPrimary
                                    font.pixelSize: app.tokens.textCaption + 1
                                    font.family: app.tokens.monoFamily
                                    elide: Text.ElideRight
                                }
                                Controls.Label {
                                    Layout.preferredWidth: page.versionColumnWidth
                                    text: modelData.version
                                    color: app.tokens.textPrimary
                                    font.pixelSize: app.tokens.textCaption + 1
                                    font.family: app.tokens.monoFamily
                                    opacity: 0.74
                                    elide: Text.ElideRight
                                }
                                Rectangle {
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: 24
                                    Layout.alignment: Qt.AlignVCenter
                                    radius: app.tokens.radiusPill
                                    color: app.tokens.accentMute
                                    Controls.Label {
                                        anchors.fill: parent
                                        anchors.leftMargin: app.tokens.spaceS
                                        anchors.rightMargin: app.tokens.spaceS
                                        verticalAlignment: Text.AlignVCenter
                                        text: modelData.license
                                        color: app.tokens.textPrimary
                                        font.pixelSize: app.tokens.textCaption
                                        font.family: app.tokens.monoFamily
                                        elide: Text.ElideRight
                                        opacity: 0.9
                                    }
                                }
                                Controls.Button {
                                    Layout.preferredWidth: page.linkColumnWidth
                                    Layout.preferredHeight: page.rowHeight - 4
                                    icon.name: "internet-services-symbolic"
                                    display: Controls.AbstractButton.IconOnly
                                    onClicked: page.openUrl(modelData.url)
                                    Controls.ToolTip.text: qsTr("Open crate project")
                                    Controls.ToolTip.visible: hovered
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
