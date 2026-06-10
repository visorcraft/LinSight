// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only

// LinSight GUI shell.
//
// Kirigami.ApplicationWindow with a left sidebar (Workspace + System
// sections, version footer) and a page-stack body. The dashboard
// model is the one shared OverviewModel — all pages bind their
// tile lists from it.

import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import com.visorcraft.LinSight

Kirigami.ApplicationWindow {
    id: app
    width: 1200
    height: 760
    minimumWidth: 920
    minimumHeight: 600
    visible: true
    title: qsTr("LinSight")

    property alias tokens: tokens
    // Page key. Plain leaves like "overview", "settings", "editor" point
    // at fixed pages; dashboard navigation uses the prefixed forms
    // `dashboard:<slug>` (read-only) and `editor:<slug>` (edit mode).
    property string currentPageKey: "overview"
    property string currentEditorSlug: ""
    property string currentViewSlug: ""

    DesignTokens { id: tokens }

    Kirigami.Theme.inherit: false
    Kirigami.Theme.colorSet: Kirigami.Theme.Window
    // Push the active LinSight theme into Kirigami.Theme at the window
    // root so every untweaked Page inherits the themed surfaces / text
    // / highlight without needing a per-page override. Pages that still
    // declare `Kirigami.Theme.inherit: false` for stylistic reasons
    // (SettingsPage, AboutPage) explicitly read from `tokens.*` so
    // they stay in sync.
    Kirigami.Theme.backgroundColor: tokens.surface0
    Kirigami.Theme.textColor:       tokens.textPrimary
    Kirigami.Theme.highlightColor:  tokens.accent
    Kirigami.Theme.highlightedTextColor: tokens.accentText
    color: tokens.surface0

    // One shared OverviewModel at app scope — Client::take_sample_rx
    // is one-shot, so a per-page model would leave every page after
    // the first stuck on "…". Pages reach it via `app.dashModel`.
    property var dashModel: theDashModel
    OverviewModel {
        id: theDashModel
        Component.onCompleted: theDashModel.start()
    }

    // PreferencesModel owns ~/.config/linsight/preferences.json (theme
    // + active dashboard slug). Constructed once at app scope so every
    // page can read `app.preferences.theme` etc.
    property var preferences: thePreferences
    PreferencesModel {
        id: thePreferences
    }

    // DashboardsModel owns ~/.config/linsight/dashboards/<slug>.json.
    // Construction triggers the legacy-dashboard.json migration.
    property var dashboards: theDashboards
    DashboardsModel {
        id: theDashboards
    }

    // HardwareModel — wraps the daemon's get_hardware / set_nickname
    // RPCs for the Hardware page. Construction is cheap; the page
    // calls reload() in Component.onCompleted so we don't hit the
    // daemon at boot for users who never open the page.
    property var hardware: theHardware
    HardwareModel {
        id: theHardware
    }

    // AlertModel — wraps the daemon's alert RPCs for the AlertsPage.
    property var alerts: theAlerts
    AlertModel {
        id: theAlerts
    }

    // HistoryModel — shared instance for the per-sensor history dialog.
    // A single instance is intentional: only one sensor's history is shown
    // at a time. QML reaches it via `app.historyModel`.
    property var historyModel: theHistoryModel
    HistoryModel {
        id: theHistoryModel
    }

    // Open the history dialog for a given sensor. Called from SensorTile
    // via app.openHistory(sensorId, label, unit).
    function openHistory(sensorId, label, unit) {
        historyDialog.openForSensor(sensorId, label, unit)
    }

    pageStack.globalToolBar.style: Kirigami.ApplicationHeaderStyle.None
    pageStack.initialPage: overviewPage

    Component.onCompleted: {
        // Pull the window to the foreground so screenshot tools and the
        // user's first frame always land on LinSight instead of whatever
        // happened to be active when the binary spawned.
        app.raise()
        app.requestActivate()
        const initial = Qt.application.arguments && Qt.application.arguments.length > 1
            ? Qt.application.arguments[1] : ""
        const known = ["overview","gpus","storage","network","hardware","alerts","editor","settings","about","licenses","credits"]
        if (initial && (known.indexOf(initial) !== -1
                        || initial.indexOf("dashboard:") === 0
                        || initial.indexOf("editor:") === 0)) {
            // Explicit CLI page wins — useful for headless screenshot
            // captures and for users who alias `linsight settings`.
            goTo(initial)
            return
        }
        // No CLI override: honour the saved Start Page preference.
        // `resolveStartPage` validates against the live dashboard
        // list and falls back to Overview if a previously-selected
        // dashboard has been deleted.
        goTo(resolveStartPage())
    }

    /// Resolve the persisted `startPage` preference into a routing
    /// key the page-stack can handle. A `dashboard:<slug>` that no
    /// longer exists on disk is downgraded to `overview` and the
    /// preference is rewritten so the next launch is clean.
    function resolveStartPage() {
        if (!app.preferences) return "overview"
        const raw = String(app.preferences.startPage || "overview")
        const workspaces = ["overview", "gpus", "storage", "network", "hardware", "alerts"]
        if (workspaces.indexOf(raw) !== -1) return raw
        if (raw.indexOf("dashboard:") === 0 && app.dashboards) {
            const slug = raw.substring("dashboard:".length)
            // Slug must (a) pass the grammar check and (b) match a
            // dashboard that still exists.
            if (app.dashboards.isValidSlug(slug)
                && app.dashboards.nameOf(slug).toString().length > 0) {
                return raw
            }
            // Saved dashboard is gone — reset the preference so a
            // future launch doesn't re-trip this fallback and the
            // Settings dropdown reflects reality.
            app.preferences.applyStartPage("overview")
        }
        return "overview"
    }

    // Resolve "editor" (no slug) to the active dashboard recorded in
    // preferences. If there are no dashboards yet, prompt to create
    // one — the editor is meaningless without a target file. Defends
    // against a poisoned `preferences.json` whose `active_dashboard`
    // field was hand-edited to a value that no longer passes the slug
    // grammar.
    function resolveActiveSlug() {
        if (!app.preferences || !app.dashboards) return ""
        const cur = app.preferences.activeDashboard
        if (cur
            && app.dashboards.isValidSlug(cur)
            && app.dashboards.nameOf(cur).toString().length > 0) {
            return cur
        }
        try {
            const list = JSON.parse(app.dashboards.summaryJson || "[]")
            if (list.length > 0) {
                const s = String(list[0].slug || "")
                if (app.dashboards.isValidSlug(s)) return s
            }
        } catch (e) {}
        return ""
    }

    // Window-scoped shortcuts.
    Shortcut { sequence: "F1"; context: Qt.ApplicationShortcut; onActivated: app.goTo("about") }
    Shortcut { sequences: [StandardKey.Preferences]; context: Qt.ApplicationShortcut; onActivated: app.goTo("settings") }
    Shortcut { sequence: "Ctrl+1"; context: Qt.ApplicationShortcut; onActivated: app.goTo("overview") }
    Shortcut { sequence: "Ctrl+2"; context: Qt.ApplicationShortcut; onActivated: app.goTo("gpus") }
    Shortcut { sequence: "Ctrl+3"; context: Qt.ApplicationShortcut; onActivated: app.goTo("storage") }
    Shortcut { sequence: "Ctrl+4"; context: Qt.ApplicationShortcut; onActivated: app.goTo("network") }
    Shortcut { sequence: "Ctrl+5"; context: Qt.ApplicationShortcut; onActivated: app.goTo("hardware") }
    Shortcut { sequence: "Ctrl+6"; context: Qt.ApplicationShortcut; onActivated: app.goTo("editor") }
    Shortcut { sequence: "Ctrl+N"; context: Qt.ApplicationShortcut; onActivated: app.openNewWindow() }
    Shortcut { sequences: [StandardKey.Quit]; context: Qt.ApplicationShortcut; onActivated: Qt.quit() }

    // Track every secondary window we open so they don't disappear into
    // the QML garbage collector. Closing a window removes itself from the
    // list via the `onClosing` handler we attach when creating it.
    property var extraWindows: []
    property int nextWindowNumber: 2

    function openNewWindow() {
        const w = Qt.createComponent(Qt.resolvedUrl("DashWindow.qml"))
        if (w.status === Component.Error) {
            console.warn("LinSight: failed to load DashWindow.qml:", w.errorString())
            return
        }
        const win = w.createObject(null, {
            "dashModel": app.dashModel,
            "windowNumber": app.nextWindowNumber,
        })
        if (win === null) {
            console.warn("LinSight: createObject returned null for DashWindow")
            return
        }
        app.nextWindowNumber += 1
        const arr = app.extraWindows.slice()
        arr.push(win)
        app.extraWindows = arr
        win.closing.connect(function() {
            const filtered = app.extraWindows.filter(function(x) { return x !== win })
            app.extraWindows = filtered
        })
    }

    function goTo(key) {
        if (key === currentPageKey) return
        // editor:<slug> — edit a specific dashboard. The slug is
        // re-validated by `DashboardsModel.isValidSlug` before any
        // file operation; reject obviously bogus URL fragments here
        // too so the page-stack never lands on a key that can't be
        // resolved.
        if (key.indexOf("editor:") === 0) {
            const slug = key.substring("editor:".length)
            if (!app.dashboards || !app.dashboards.isValidSlug(slug)) {
                console.warn("LinSight: rejecting unsafe editor slug:", JSON.stringify(slug))
                return
            }
            currentEditorSlug = slug
            currentPageKey = key
            if (app.preferences) app.preferences.applyActiveDashboard(slug)
            app.pageStack.replace(editorPage)
            return
        }
        // dashboard:<slug> — view a specific dashboard.
        if (key.indexOf("dashboard:") === 0) {
            const slug = key.substring("dashboard:".length)
            if (!app.dashboards || !app.dashboards.isValidSlug(slug)) {
                console.warn("LinSight: rejecting unsafe dashboard slug:", JSON.stringify(slug))
                return
            }
            currentViewSlug = slug
            currentPageKey = key
            if (app.preferences) app.preferences.applyActiveDashboard(slug)
            app.pageStack.replace(dashboardViewPage)
            return
        }
        if (key === "editor") {
            // Bare "editor" — resolve to the active dashboard. If there
            // are none, prompt to create one rather than landing on a
            // page with nothing to save.
            const slug = resolveActiveSlug()
            if (slug.length === 0) {
                newDashboardDialog.open()
                return
            }
            goTo("editor:" + slug)
            return
        }
        currentPageKey = key
        switch (key) {
            case "overview": app.pageStack.replace(overviewPage); break
            case "gpus":     app.pageStack.replace(gpusPage); break
            case "storage":  app.pageStack.replace(storagePage); break
            case "network":  app.pageStack.replace(networkPage); break
            case "hardware": app.pageStack.replace(hardwarePage); break
            case "alerts":   app.pageStack.replace(alertsPage); break
            case "settings": app.pageStack.replace(settingsPage); break
            case "about":    app.pageStack.replace(aboutPage); break
            case "licenses": app.pageStack.replace(licensesPage); break
            case "credits":  app.pageStack.replace(creditsPage); break
        }
    }

    globalDrawer: Kirigami.GlobalDrawer {
        id: drawer
        edge: Qt.LeftEdge
        modal: false
        drawerOpen: true
        collapsible: true
        collapsed: false
        width: drawer.isCollapsed ? Kirigami.Units.gridUnit * 3
                                  : Kirigami.Units.gridUnit * 14
        Behavior on width { NumberAnimation { duration: tokens.durationSnap; easing.type: Easing.OutCubic } }
        handleVisible: false

        readonly property bool isCollapsed: drawer.collapsible && drawer.collapsed

        background: Rectangle {
            color: tokens.surfaceSidebar
            Rectangle {
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: 1
                color: tokens.separator
            }
        }

        contentItem: ColumnLayout {
            id: drawerColumn
            spacing: 0

            // Header — collapse toggle + app brand
            RowLayout {
                Layout.fillWidth: true
                Layout.preferredHeight: 64
                Layout.topMargin: tokens.spaceL
                Layout.leftMargin: drawer.isCollapsed ? 0 : tokens.spaceL
                Layout.rightMargin: drawer.isCollapsed ? 0 : tokens.spaceL
                Layout.bottomMargin: tokens.spaceL
                spacing: tokens.spaceM

                Controls.ToolButton {
                    Layout.alignment: drawer.isCollapsed
                                          ? Qt.AlignHCenter | Qt.AlignVCenter
                                          : Qt.AlignVCenter
                    Layout.fillWidth: drawer.isCollapsed
                    icon.name: "application-menu-symbolic"
                    icon.color: tokens.textPrimary
                    display: Controls.AbstractButton.IconOnly
                    Controls.ToolTip.text: drawer.isCollapsed ? qsTr("Open Sidebar")
                                                              : qsTr("Close Sidebar")
                    Controls.ToolTip.visible: hovered
                    Controls.ToolTip.delay: 400
                    Accessible.name: Controls.ToolTip.text
                    onClicked: drawer.collapsed = !drawer.collapsed
                }
                Rectangle {
                    Layout.preferredWidth: 40
                    Layout.preferredHeight: 40
                    radius: tokens.radiusAvatar
                    color: "transparent"
                    visible: !drawer.isCollapsed
                    Image {
                        anchors.fill: parent
                        source: "qrc:/qt/qml/com/visorcraft/LinSight/resources/linsight-128.png"
                        sourceSize.width: 80
                        sourceSize.height: 80
                        smooth: true
                        mipmap: true
                        // Falls back invisibly when the resource isn't present
                        // — first launch on a dev build without packaged icons.
                        onStatusChanged: if (status === Image.Error) visible = false
                    }
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1
                    visible: !drawer.isCollapsed
                    Controls.Label {
                        text: "LinSight"
                        font.pixelSize: tokens.textSubheading + 1
                        font.weight: tokens.weightBold
                        font.family: tokens.sansFamily
                        color: tokens.textPrimary
                    }
                    Controls.Label {
                        text: qsTr("Multi-GPU system insight")
                        font.pixelSize: tokens.textCaption
                        opacity: 0.55
                        color: tokens.textPrimary
                    }
                }
            }

            // Workspace section
            Controls.Label {
                Layout.fillWidth: true
                Layout.leftMargin: tokens.spaceL
                Layout.rightMargin: tokens.spaceL
                Layout.topMargin: tokens.spaceS
                Layout.bottomMargin: tokens.spaceS
                text: qsTr("WORKSPACE")
                font.pixelSize: 10
                font.weight: tokens.weightSemibold
                opacity: 0.5
                visible: !drawer.isCollapsed
                color: tokens.textPrimary
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("Overview")
                iconName: "view-grid-symbolic"
                active: app.currentPageKey === "overview"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("overview")
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("GPUs")
                iconName: "video-display-symbolic"
                active: app.currentPageKey === "gpus"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("gpus")
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("Storage")
                iconName: "drive-harddisk-symbolic"
                active: app.currentPageKey === "storage"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("storage")
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("Network")
                iconName: "network-wired-symbolic"
                active: app.currentPageKey === "network"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("network")
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("Hardware")
                iconName: "preferences-other"
                active: app.currentPageKey === "hardware"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("hardware")
            }
            // No standalone Editor nav item. Editing is bound to a
            // specific dashboard: clicking a DASHBOARDS row opens its
            // read-only view, which has an "Edit" affordance; the New
            // Dashboard flow opens the editor on the freshly-created
            // file. Ctrl+5 still works (resolves to the active slug).
            NavItem {
                Layout.fillWidth: true
                label: qsTr("New Window")
                iconName: "window-new-symbolic"
                active: false
                compact: drawer.isCollapsed
                onTriggered: app.openNewWindow()
            }

            // Dashboards section. Each saved dashboard becomes a nav row
            // that opens it in read-only view. The list refreshes when
            // DashboardsModel.summaryJsonChanged fires (created /
            // renamed / removed). The "+ New" row at the bottom is
            // always visible so first-launch users have an obvious path
            // to create one when no dashboards exist yet.
            Controls.Label {
                Layout.fillWidth: true
                Layout.leftMargin: tokens.spaceL
                Layout.rightMargin: tokens.spaceL
                Layout.topMargin: tokens.spaceXL
                Layout.bottomMargin: tokens.spaceS
                text: qsTr("DASHBOARDS")
                font.pixelSize: 10
                font.weight: tokens.weightSemibold
                opacity: 0.5
                visible: !drawer.isCollapsed
                color: tokens.textPrimary
            }
            // Routed through a Connection-backed `var` property so the
            // Repeater re-evaluates when DashboardsModel emits
            // summaryJsonChanged (created / renamed / removed).
            property var dashboardEntries: []
            function refreshDashboardEntries() {
                if (!app.dashboards) { drawerColumn.dashboardEntries = []; return }
                try {
                    drawerColumn.dashboardEntries =
                        JSON.parse(app.dashboards.summaryJson || "[]")
                } catch (e) {
                    drawerColumn.dashboardEntries = []
                }
            }
            Component.onCompleted: drawerColumn.refreshDashboardEntries()
            Connections {
                target: app.dashboards
                function onSummaryJsonChanged() {
                    drawerColumn.refreshDashboardEntries()
                }
            }
            Repeater {
                model: drawerColumn.dashboardEntries
                delegate: NavItem {
                    Layout.fillWidth: true
                    label: modelData.name
                    iconName: "view-presentation-symbolic"
                    active: app.currentPageKey === "dashboard:" + modelData.slug
                    compact: drawer.isCollapsed
                    onTriggered: app.goTo("dashboard:" + modelData.slug)
                }
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("Alerts")
                iconName: "dialog-warning-symbolic"
                active: app.currentPageKey === "alerts"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("alerts")
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("New Dashboard")
                iconName: "list-add-symbolic"
                active: false
                compact: drawer.isCollapsed
                onTriggered: newDashboardDialog.open()
            }

            // System section
            Controls.Label {
                Layout.fillWidth: true
                Layout.leftMargin: tokens.spaceL
                Layout.rightMargin: tokens.spaceL
                Layout.topMargin: tokens.spaceXL
                Layout.bottomMargin: tokens.spaceS
                text: qsTr("SYSTEM")
                font.pixelSize: 10
                font.weight: tokens.weightSemibold
                opacity: 0.5
                visible: !drawer.isCollapsed
                color: tokens.textPrimary
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("Settings")
                iconName: "settings-configure-symbolic"
                active: app.currentPageKey === "settings"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("settings")
            }
            NavItem {
                Layout.fillWidth: true
                label: qsTr("About")
                iconName: "help-about-symbolic"
                active: app.currentPageKey === "about"
                    || app.currentPageKey === "licenses"
                    || app.currentPageKey === "credits"
                compact: drawer.isCollapsed
                onTriggered: app.goTo("about")
            }

            Item { Layout.fillHeight: true; Layout.fillWidth: true }

            // Footer — version pill (or compact strip)
            Controls.Label {
                Layout.fillWidth: true
                Layout.leftMargin: tokens.spaceXS
                Layout.rightMargin: tokens.spaceXS
                Layout.bottomMargin: tokens.spaceL
                horizontalAlignment: Text.AlignHCenter
                text: "v" + Qt.application.version
                font.pixelSize: tokens.textCaption - 1
                minimumPixelSize: 8
                fontSizeMode: Text.HorizontalFit
                font.family: tokens.monoFamily
                opacity: 0.65
                color: tokens.textPrimary
                visible: drawer.isCollapsed
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.leftMargin: tokens.spaceL
                Layout.rightMargin: tokens.spaceL
                Layout.bottomMargin: tokens.spaceL
                Layout.topMargin: tokens.spaceM
                spacing: tokens.spaceS
                visible: !drawer.isCollapsed
                Rectangle {
                    radius: tokens.radiusPill
                    color: tokens.pillBackground
                    border.color: tokens.separator
                    border.width: 1
                    implicitHeight: 22
                    implicitWidth: versionLabel.implicitWidth + tokens.spaceM * 2
                    Controls.Label {
                        id: versionLabel
                        anchors.centerIn: parent
                        text: "v" + Qt.application.version
                        font.pixelSize: tokens.textCaption
                        font.family: tokens.monoFamily
                        opacity: 0.7
                        color: tokens.textPrimary
                    }
                }
            }
        }
    }

    // Disconnected banner: visible whenever the OverviewModel reports it
    // is not currently receiving samples from the daemon. Without this,
    // tiles freeze at their last value and the user has no idea anything
    // is wrong. Anchored at the top of the window so it works for every
    // page; sits above the page content via a higher z.
    Rectangle {
        z: 1000
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: visible ? 28 : 0
        color: tokens.surface2 !== undefined ? tokens.surface2 : "#7a1c1c"
        visible: app.dashModel !== null && app.dashModel.connected === false
        Controls.Label {
            anchors.centerIn: parent
            text: qsTr("Disconnected from linsightd — values shown are last known.")
            color: tokens.textPrimary
            font.pixelSize: tokens.textCaption
        }
    }

    Component { id: overviewPage;  OverviewPage  { dashModel: app.dashModel } }
    Component { id: gpusPage;      CategoryPage  { dashModel: app.dashModel; category: "gpu";     pageTitle: qsTr("GPUs"); groupBy: "deviceLabel" } }
    Component { id: storagePage;   CategoryPage  { dashModel: app.dashModel; category: "storage"; pageTitle: qsTr("Storage"); groupBy: "deviceLabel" } }
    Component { id: networkPage;   CategoryPage  { dashModel: app.dashModel; category: "network"; pageTitle: qsTr("Network") } }
    Component { id: hardwarePage;  HardwarePage  {} }
    Component { id: alertsPage;    AlertsPage    { alertModel: app.alerts; dashModel: app.dashModel } }
    Component { id: editorPage;        CanvasEditorPage  { dashModel: app.dashModel; editingSlug: app.currentEditorSlug } }
    Component { id: dashboardViewPage; DashboardViewPage { dashModel: app.dashModel; viewingSlug: app.currentViewSlug } }
    Component { id: settingsPage;  SettingsPage  { dashModel: app.dashModel } }
    Component {
        id: aboutPage
        AboutPage {
            onNavigateRequested: pageKey => app.goTo(pageKey)
        }
    }
    Component { id: licensesPage;  LicensesPage  { dashModel: app.dashModel; onGplTextRequested: gplLicenseDialog.open() } }
    Component { id: creditsPage;   CreditsPage   { dashModel: app.dashModel } }

    GplLicenseDialog { id: gplLicenseDialog; dashModel: app.dashModel }

    HistoryDialog {
        id: historyDialog
        anchors.centerIn: parent
        historyModel: app.historyModel
    }

    NewDashboardDialog {
        id: newDashboardDialog
        anchors.centerIn: parent
        onDashboardCreated: slug => app.goTo("editor:" + slug)
        onDashboardFailed: detail => {
            createFailedBanner.text = detail
            createFailedBanner.visible = true
            bannerHideTimer.restart()
        }
    }

    // Window-level failure banner for ops triggered from the sidebar
    // / dialogs (where no page-level banner exists). Mirrors the
    // CanvasEditorPage convention of discriminating success vs error
    // by type rather than message contents.
    Kirigami.InlineMessage {
        id: createFailedBanner
        z: 1100
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.topMargin: 32
        anchors.leftMargin: tokens.spaceL
        anchors.rightMargin: tokens.spaceL
        type: Kirigami.MessageType.Error
        visible: false
        showCloseButton: true
    }
    Timer {
        id: bannerHideTimer
        interval: 5000
        repeat: false
        onTriggered: createFailedBanner.visible = false
    }
}
