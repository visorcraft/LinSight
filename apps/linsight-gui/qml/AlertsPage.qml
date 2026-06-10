// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// Alert Rules page — list, add, test, and delete alert rules.
// Communicates with the daemon via AlertModel RPC proxy.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Page {
    id: page
    title: qsTr("Alerts")
    padding: 0
    Accessible.role: Accessible.Pane
    Accessible.name: qsTr("Alert Rules")

    Rectangle { anchors.fill: parent; color: app.tokens.surface0; z: -1 }

    property QtObject alertModel: null
    property QtObject dashModel: null

    property var rules: []
    property string testResultText: ""

    Component.onCompleted: if (page.alertModel) page.alertModel.reload()

    Connections {
        target: page.alertModel
        function onRulesJsonChanged() { page.parseRules() }
    }

    function parseRules() {
        if (!page.alertModel) return
        try {
            const arr = JSON.parse(page.alertModel.rulesJson || "[]")
            page.rules = Array.isArray(arr) ? arr : []
        } catch (e) {
            page.rules = []
        }
    }

    function sensorIds() {
        if (!page.dashModel) return []
        try {
            const tiles = JSON.parse(page.dashModel.tilesJson || "[]")
            return tiles.map(function(t) { return t.id }).sort()
        } catch (e) {
            return []
        }
    }

    // ── Header ──────────────────────────────────────────────────
    Rectangle {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: app.tokens.pageHeaderHeight
        color: app.tokens.surface0
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 1
            color: app.tokens.separator
        }
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: app.tokens.spaceXL
            anchors.rightMargin: app.tokens.spaceXL
            spacing: app.tokens.spaceM
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1
                Controls.Label {
                    text: qsTr("Alert Rules")
                    font.pixelSize: app.tokens.textHeading
                    font.weight: app.tokens.weightBold
                    font.family: app.tokens.sansFamily
                    color: app.tokens.textPrimary
                }
                Controls.Label {
                    text: qsTr("Set conditions that trigger notifications")
                    opacity: 0.6
                    font.pixelSize: app.tokens.textCaption + 1
                    color: app.tokens.textPrimary
                }
            }
            ThemedButton {
                icon.name: "list-add-symbolic"
                text: qsTr("Add Rule")
                onClicked: editDialog.openNew()
            }
            ThemedButton {
                icon.name: "view-refresh-symbolic"
                text: qsTr("Reload")
                onClicked: if (page.alertModel) page.alertModel.reload()
            }
        }
    }

    // ── Rule list ───────────────────────────────────────────────
    Rectangle {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        color: app.tokens.surface0
        clip: true

        Controls.Label {
            anchors.centerIn: parent
            visible: !page.alertModel.isLoading && page.rules.length === 0
            text: qsTr("No alert rules configured.\nAdd one using the button above.")
            opacity: 0.5
            color: app.tokens.textPrimary
            font.pixelSize: app.tokens.textBody
            horizontalAlignment: Text.AlignHCenter
        }

        Kirigami.InlineMessage {
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            visible: page.alertModel && page.alertModel.lastError.length > 0
            text: page.alertModel ? page.alertModel.lastError : ""
            type: Kirigami.MessageType.Error
            showCloseButton: true
            z: 10
        }

        Controls.BusyIndicator {
            anchors.centerIn: parent
            visible: page.alertModel && page.alertModel.isLoading
        }

        Controls.ScrollView {
            anchors.fill: parent
            clip: true
            visible: page.rules.length > 0

            ColumnLayout {
                width: parent.width
                spacing: app.tokens.spaceS
                anchors.margins: app.tokens.spaceM

                Repeater {
                    model: page.rules
                    delegate: Rectangle {
                        Layout.fillWidth: true
                        implicitHeight: ruleBody.implicitHeight + app.tokens.spaceXL * 2
                        radius: app.tokens.radiusCard
                        color: app.tokens.surface1
                        border.width: 1
                        border.color: app.tokens.separator
                        opacity: modelData.enabled === false ? 0.55 : 1.0

                        RowLayout {
                            id: ruleBody
                            anchors.fill: parent
                            anchors.margins: app.tokens.spaceM
                            spacing: app.tokens.spaceM

                            Controls.Switch {
                                checked: modelData.enabled !== false
                                onToggled: {
                                    if (!page.alertModel) return
                                    page.alertModel.upsert(
                                        modelData.name,
                                        modelData.expr,
                                        (modelData.notify || []).join(", "),
                                        checked ? 1 : -1,
                                        ""
                                    )
                                }
                                Layout.alignment: Qt.AlignVCenter
                            }

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: app.tokens.spaceXS

                                Controls.Label {
                                    text: modelData.name || ""
                                    font.pixelSize: app.tokens.textBody
                                    font.weight: app.tokens.weightBold
                                    color: app.tokens.textPrimary
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }

                                Controls.Label {
                                    text: modelData.expr || ""
                                    font.family: app.tokens.monoFamily
                                    font.pixelSize: app.tokens.textCaption
                                    color: app.tokens.textSecondary
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }

                                RowLayout {
                                    spacing: app.tokens.spaceXS
                                    visible: modelData.notify && modelData.notify.length > 0
                                    Controls.Label {
                                        text: qsTr("Notify:")
                                        font.pixelSize: app.tokens.textCaption
                                        opacity: 0.6
                                        color: app.tokens.textPrimary
                                    }
                                    Repeater {
                                        model: modelData.notify || []
                                        delegate: Controls.Label {
                                            text: modelData
                                            font.pixelSize: app.tokens.textCaption
                                            color: app.tokens.accent || "#4fc3f7"
                                        }
                                    }
                                }
                            }

                            Controls.Button {
                                icon.name: "media-playback-start-symbolic"
                                text: qsTr("Test")
                                flat: true
                                onClicked: {
                                    if (!page.alertModel) return
                                    page.alertModel.test_expr(modelData.expr)
                                    testResultDialog.exprText = modelData.expr
                                    testResultDialog.open()
                                }
                            }

                            Controls.Button {
                                icon.name: "document-edit-symbolic"
                                text: qsTr("Edit")
                                flat: true
                                onClicked: editDialog.openEdit(
                                    modelData.name,
                                    modelData.expr,
                                    (modelData.notify || []).join(", ")
                                )
                            }

                            Controls.Button {
                                icon.name: "edit-delete-symbolic"
                                text: qsTr("Delete")
                                flat: true
                                onClicked: deleteConfirmDialog.prepare(modelData.name)
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Add/Edit dialog ─────────────────────────────────────────
    Kirigami.Dialog {
        id: editDialog
        title: qsTr("Add Alert Rule")
        standardButtons: Kirigami.Dialog.Save | Kirigami.Dialog.Cancel

        property string editingName: ""

        function openNew() {
            editingName = ""
            nameField.text = ""
            exprField.text = ""
            notifyField.text = ""
            sensorPicker.currentIndex = -1
            title = qsTr("Add Alert Rule")
            open()
        }

        function openEdit(name, expr, notify) {
            editingName = name
            nameField.text = name
            exprField.text = expr
            notifyField.text = notify
            sensorPicker.currentIndex = -1
            title = qsTr("Edit Rule: %1").arg(name)
            open()
        }

        onAccepted: {
            const name = nameField.text.trim()
            const expr = exprField.text.trim()
            if (!name || !expr) {
                app.showPassiveNotification(qsTr("Name and expression are required."), 3000)
                return
            }
            const notifyStr = notifyField.text.trim()
            if (page.alertModel) {
                // 0 = preserve current enabled flag; the edit dialog
                // doesn't surface enable/disable, so we shouldn't touch it.
                page.alertModel.upsert(name, expr, notifyStr, 0, "")
            }
            app.showPassiveNotification(qsTr("Saving rule '%1'...").arg(name), 2000)
        }

        ColumnLayout {
            spacing: app.tokens.spaceM
            Layout.fillWidth: true
            implicitWidth: 440

            Controls.Label {
                text: qsTr("Rule Name")
                font.weight: app.tokens.weightSemibold
                color: app.tokens.textPrimary
            }
            Controls.TextField {
                id: nameField
                Layout.fillWidth: true
                placeholderText: qsTr("e.g. high-cpu")
            }

            Controls.Label {
                text: qsTr("Expression")
                font.weight: app.tokens.weightSemibold
                color: app.tokens.textPrimary
            }
            Controls.TextField {
                id: exprField
                Layout.fillWidth: true
                placeholderText: qsTr("e.g. cpu.util > 90 && mem.used_bytes > 8e9")
                font.family: app.tokens.monoFamily
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceXS

                Controls.Label {
                    text: qsTr("Insert:")
                    font.pixelSize: app.tokens.textCaption
                    opacity: 0.7
                    color: app.tokens.textPrimary
                }

                Controls.Button {
                    text: "&&"
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, " && ")
                }
                Controls.Button {
                    text: "||"
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, " || ")
                }
                Controls.Button {
                    text: "!"
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, "!")
                }
                Controls.Button {
                    text: ">"
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, " > ")
                }
                Controls.Button {
                    text: "<"
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, " < ")
                }
                Controls.Button {
                    text: ">="
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, " >= ")
                }
                Controls.Button {
                    text: "<="
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, " <= ")
                }
                Controls.Button {
                    text: "("
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, "(")
                }
                Controls.Button {
                    text: ")"
                    font.family: app.tokens.monoFamily
                    font.pixelSize: app.tokens.textCaption
                    flat: true
                    onClicked: exprField.insert(exprField.cursorPosition, ")")
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: app.tokens.spaceS

                Controls.Label {
                    text: qsTr("Sensor:")
                    font.pixelSize: app.tokens.textCaption
                    opacity: 0.7
                    color: app.tokens.textPrimary
                }

                Controls.ComboBox {
                    id: sensorPicker
                    Layout.fillWidth: true
                    textRole: "text"
                    valueRole: "value"
                    model: {
                        var ids = page.sensorIds()
                        var m = []
                        for (var i = 0; i < ids.length; i++) {
                            m.push({ text: ids[i], value: ids[i] })
                        }
                        return m
                    }
                    onActivated: {
                        if (currentIndex >= 0) {
                            var id = currentValue
                            exprField.insert(exprField.cursorPosition, id)
                            currentIndex = -1
                        }
                    }
                }

                Controls.Button {
                    text: qsTr("Test")
                    icon.name: "media-playback-start-symbolic"
                    onClicked: {
                        if (!exprField.text.trim() || !page.alertModel) return
                        page.alertModel.test_expr(exprField.text.trim())
                        testResultDialog.exprText = exprField.text.trim()
                        testResultDialog.open()
                    }
                }
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                visible: syntaxHelpToggle.checked
                type: Kirigami.MessageType.Information
                showCloseButton: false

                ColumnLayout {
                    spacing: app.tokens.spaceXS
                    Controls.Label {
                        text: qsTr("Expression Syntax")
                        font.weight: app.tokens.weightSemibold
                        color: app.tokens.textPrimary
                    }
                    Controls.Label {
                        Layout.fillWidth: true
                        wrapMode: Text.WordWrap
                        font.family: app.tokens.monoFamily
                        font.pixelSize: app.tokens.textCaption
                        color: app.tokens.textPrimary
                        text: qsTr(
                            "Sensor IDs are variables (e.g. <b>cpu.util</b>, <b>mem.used_bytes</b>).<br>" +
                            "Compare: <b>></b> <b><</b> <b>>=</b> <b><=</b> <b>==</b> <b>!=</b><br>" +
                            "Combine: <b>&&</b> (AND) <b>||</b> (OR) <b>!</b> (NOT)<br>" +
                            "Group: <b>( )</b> parentheses for precedence<br><br>" +
                            "Examples:<br>" +
                            "  cpu.util > 90<br>" +
                            "  cpu.util > 80 && mem.used_bytes > 8e9<br>" +
                            "  (xe.gpu0.temp_c > 85 || xe.gpu1.temp_c > 85)<br>" +
                            "  !(cpu.util > 10) && system.load_1m > 4"
                        )
                    }
                }
            }

            Controls.Button {
                id: syntaxHelpToggle
                checkable: true
                flat: true
                icon.name: "help-contents-symbolic"
                text: checked ? qsTr("Hide Syntax Help") : qsTr("Show Syntax Help")
                font.pixelSize: app.tokens.textCaption
            }

            Controls.Label {
                text: qsTr("Notification Targets")
                font.weight: app.tokens.weightSemibold
                color: app.tokens.textPrimary
            }
            Controls.TextField {
                id: notifyField
                Layout.fillWidth: true
                placeholderText: qsTr("desktop, webhook:https://...")
            }

            Controls.Label {
                text: qsTr("Comma-separated. Supported: desktop, exec:/path, webhook:url")
                font.pixelSize: app.tokens.textCaption
                opacity: 0.6
                color: app.tokens.textPrimary
            }
        }
    }

    // ── Test result dialog ──────────────────────────────────────
    Kirigami.Dialog {
        id: testResultDialog
        title: qsTr("Test Expression")
        standardButtons: Kirigami.Dialog.Ok

        property string exprText: ""

        Controls.Label {
            Layout.fillWidth: true
            text: page.alertModel && page.alertModel.testResult.length > 0
                  ? page.alertModel.testResult
                  : qsTr("Waiting for result...")
            wrapMode: Text.WordWrap
            color: app.tokens.textPrimary
        }
    }

    // ── Delete confirmation ─────────────────────────────────────
    Kirigami.Dialog {
        id: deleteConfirmDialog
        title: qsTr("Delete Rule")
        standardButtons: Kirigami.Dialog.Yes | Kirigami.Dialog.Cancel

        property string deletingName: ""

        function prepare(name) {
            deletingName = name
            messageLabel.text = qsTr("Delete rule '%1'?").arg(name)
            open()
        }

        Controls.Label {
            id: messageLabel
            color: app.tokens.textPrimary
        }

        onAccepted: {
            if (!deletingName || !page.alertModel) return
            page.alertModel.deleteRule(deletingName)
            app.showPassiveNotification(qsTr("Deleted rule '%1'").arg(deletingName), 3000)
        }
    }
}
