import QtQuick
import QtQuick.Layouts
import Quickshell.Io

Item {
    id: root

    property color bgSecondary: "#24242c"
    property color textColor: "#d4d4dc"
    property color textDim: "#868690"
    property color accentColor: "#7c7c88"
    property int pollInterval: 5000

    property int watchSeconds: 0
    property int trackedSeconds: 0
    property string updatedAt: ""
    property string date: ""
    property var apps: []
    property var domains: []
    property var categories: []
    property var days: []
    property bool stale: true
    property int activeSessionSeconds: 0
    property bool breakOverdue: false
    property bool paused: false
    property string pausedReason: ""
    property string pausedSince: ""
    property bool breaksEnabled: true
    property int breaksIntervalSecs: 3600
    property int breaksMinBreakSecs: 300
    property var uncategorizedApps: []
    property var uncategorizedDomains: []
    property bool notificationsEnabled: true
    property bool notificationsBreakOverdue: true
    property bool notificationsBudgetExceeded: true
    property string focusSourceKind: "auto"

    signal clicked()

    property int _initialPolls: 0

    visible: true
    implicitWidth: chip.implicitWidth
    implicitHeight: 30
    width: implicitWidth
    height: implicitHeight

    function refresh() {
        if (!attnStatus.running) {
            attnStatus.running = true;
        }
    }

    Timer {
        id: pollTimer
        interval: root._initialPolls < 3 ? 800 : root.pollInterval
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            root.refresh();
            if (root._initialPolls < 3) root._initialPolls += 1;
        }
    }

    Process {
        id: attnStatus
        command: ["attn", "status", "--json"]
        onExited: {
            if (root.updatedAt === "") {
                root.stale = true;
            }
        }
        stdout: StdioCollector {
            onStreamFinished: {
                var raw = this.text.trim();
                if (!raw) {
                    // Transient failure (timeout, busy daemon). Keep last good
                    // values so the widget doesn't blink to empty; flag stale.
                    root.stale = true;
                    return;
                }
                try {
                    var status = JSON.parse(raw);
                    root.watchSeconds = Number(status.watch_seconds || 0);
                    root.trackedSeconds = Number(status.tracked_seconds || 0);
                    root.updatedAt = String(status.updated_at || "");
                    root.date = String(status.date || "");
                    root.apps = status.apps || [];
                    root.domains = status.domains || [];
                    root.categories = status.categories || [];
                    root.days = status.days || [];
                    root.activeSessionSeconds = Number(status.active_session_seconds || 0);
                    root.breakOverdue = !!status.break_overdue;
                    root.paused = !!status.paused;
                    root.pausedReason = String(status.paused_reason || "");
                    root.pausedSince = String(status.paused_since || "");
                    if (status.breaks_enabled !== undefined) root.breaksEnabled = !!status.breaks_enabled;
                    if (status.breaks_interval_secs !== undefined) root.breaksIntervalSecs = Number(status.breaks_interval_secs);
                    if (status.breaks_min_break_secs !== undefined) root.breaksMinBreakSecs = Number(status.breaks_min_break_secs);
                    root.uncategorizedApps = status.uncategorized_apps || [];
                    root.uncategorizedDomains = status.uncategorized_domains || [];
                    if (status.notifications_enabled !== undefined) root.notificationsEnabled = !!status.notifications_enabled;
                    if (status.notifications_break_overdue !== undefined) root.notificationsBreakOverdue = !!status.notifications_break_overdue;
                    if (status.notifications_budget_exceeded !== undefined) root.notificationsBudgetExceeded = !!status.notifications_budget_exceeded;
                    if (status.focus_source_kind !== undefined) root.focusSourceKind = String(status.focus_source_kind);
                    root.stale = false;
                } catch (e) {
                    root.stale = true;
                }
            }
        }
    }

    Rectangle {
        id: chip
        anchors.fill: parent
        implicitWidth: 30
        implicitHeight: 30
        radius: 15
        readonly property bool alert: root.breakOverdue || root.paused
        color: chipArea.containsMouse ? Qt.lighter(root.bgSecondary, 1.2) : root.bgSecondary
        border.width: 1
        border.color: chip.alert ? "#c9b563" : root.accentColor
        opacity: root.stale ? 0.45 : 1.0

        Behavior on color { ColorAnimation { duration: 120; easing.type: Easing.OutCubic } }
        Behavior on border.color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }

        SequentialAnimation on opacity {
            running: chip.alert && !root.stale
            loops: Animation.Infinite
            NumberAnimation { to: 0.55; duration: 800; easing.type: Easing.InOutSine }
            NumberAnimation { to: 1.0;  duration: 800; easing.type: Easing.InOutSine }
        }

        Item {
            id: content
            anchors.centerIn: parent
            width: 18
            height: 18

            Text {
                anchors.centerIn: parent
                text: "◷"
                color: root.textColor
                font.pixelSize: 16
                font.weight: Font.DemiBold
            }

            Rectangle {
                anchors.right: parent.right
                anchors.top: parent.top
                width: 6
                height: 6
                radius: 3
                color: "#d8b45c"
                visible: root.trackedSeconds > 0
            }
        }

        MouseArea {
            id: chipArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: root.clicked()
        }
    }
}
