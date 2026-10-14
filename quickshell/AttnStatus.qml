import QtQuick
import Quickshell.Io

// The daemon's status and every command the widget sends back, in one object.
// AttnIndicator owns one by default; hand that same object to AttnPopup, so a
// bar wires one property instead of mirroring each status field.
Item {
    id: root

    property int pollInterval: 5000

    // ---- status, as `attn status --json` reports it ------------------------
    // Fields the daemon omits when empty fall back to these defaults.
    property bool stale: true
    property int watchSeconds: 0
    property int trackedSeconds: 0
    property string updatedAt: ""
    property string date: ""
    property var apps: []
    property var domains: []
    property var categories: []
    property var days: []
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
    property int switchCount: 0
    property int longestFocusSecs: 0
    property int deepWorkBlocks: 0
    property string activeSince: ""
    property string activeUntil: ""

    // Set by setFocusSource: the daemon reads a new kind only at start.
    property string restartHint: ""

    function refresh() {
        if (!statusProc.running)
            statusProc.running = true;
    }

    function breakStart() { run(["break-start"]); }
    function breakEnd() { run(["break-end"]); }

    function setBreaks(enabled, intervalSecs, minBreakSecs) {
        run(["set-breaks", "--enabled=" + enabled, "--interval=" + intervalSecs, "--min-break=" + minBreakSecs]);
    }

    function setNotifications(enabled, breakOverdue, budgetExceeded) {
        run(["set-notifications", "--enabled=" + enabled, "--break-overdue=" + breakOverdue, "--budget-exceeded=" + budgetExceeded]);
    }

    function setFocusSource(kind) {
        run(["set-focus-source", "--kind=" + kind]);
        restartHint = "Focus source saved — restart the daemon to apply.";
    }

    function setBudget(category, secs) {
        run(["set-budget", "--category=" + category, "--secs=" + secs]);
    }

    function categorize(kind, id, category) {
        run(["categorize", "--kind=" + kind, "--id=" + id, "--category=" + category]);
    }

    // Commands run one at a time, in click order. A second click while the
    // first is still running is queued, not dropped.
    property var _queue: []

    function run(args) {
        _queue = _queue.concat([args]);
        if (!commandProc.running)
            _next();
    }

    function _next() {
        if (_queue.length === 0)
            return;
        commandProc.command = ["attn"].concat(_queue[0]);
        _queue = _queue.slice(1);
        commandProc.running = true;
    }

    Process {
        id: commandProc
        onExited: {
            root.refresh();
            root._next();
        }
    }

    // Fast at first so the chip fills in right after login, then settled.
    property int _initialPolls: 0

    Timer {
        interval: root._initialPolls < 3 ? 800 : root.pollInterval
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            root.refresh();
            if (root._initialPolls < 3)
                root._initialPolls += 1;
        }
    }

    Process {
        id: statusProc
        command: ["attn", "status", "--json"]
        onExited: if (root.updatedAt === "")
            root.stale = true

        stdout: StdioCollector {
            onStreamFinished: {
                var raw = this.text.trim();
                if (!raw) {
                    // Transient failure (timeout, busy daemon). Keep the last
                    // good values so the widget does not blink empty.
                    root.stale = true;
                    return;
                }
                try {
                    root.apply(JSON.parse(raw));
                } catch (e) {
                    root.stale = true;
                }
            }
        }
    }

    function apply(s) {
        watchSeconds = Number(s.watch_seconds || 0);
        trackedSeconds = Number(s.tracked_seconds || 0);
        updatedAt = String(s.updated_at || "");
        date = String(s.date || "");
        apps = s.apps || [];
        domains = s.domains || [];
        categories = s.categories || [];
        days = s.days || [];
        activeSessionSeconds = Number(s.active_session_seconds || 0);
        breakOverdue = !!s.break_overdue;
        paused = !!s.paused;
        pausedReason = String(s.paused_reason || "");
        pausedSince = String(s.paused_since || "");
        if (s.breaks_enabled !== undefined) breaksEnabled = !!s.breaks_enabled;
        if (s.breaks_interval_secs !== undefined) breaksIntervalSecs = Number(s.breaks_interval_secs);
        if (s.breaks_min_break_secs !== undefined) breaksMinBreakSecs = Number(s.breaks_min_break_secs);
        uncategorizedApps = s.uncategorized_apps || [];
        uncategorizedDomains = s.uncategorized_domains || [];
        if (s.notifications_enabled !== undefined) notificationsEnabled = !!s.notifications_enabled;
        if (s.notifications_break_overdue !== undefined) notificationsBreakOverdue = !!s.notifications_break_overdue;
        if (s.notifications_budget_exceeded !== undefined) notificationsBudgetExceeded = !!s.notifications_budget_exceeded;
        if (s.focus_source_kind !== undefined) focusSourceKind = String(s.focus_source_kind);
        switchCount = Number(s.switch_count || 0);
        longestFocusSecs = Number(s.longest_focus_secs || 0);
        deepWorkBlocks = Number(s.deep_work_blocks || 0);
        activeSince = String(s.active_since || "");
        activeUntil = String(s.active_until || "");
        stale = false;
    }
}
