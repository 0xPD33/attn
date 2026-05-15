import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic

PopupWindow {
    id: popup

    required property color bgColor
    required property color bgSecondary
    required property color textColor
    required property color textDim
    required property color accentColor

    property string fontFamily: "JetBrainsMono Nerd Font"
    readonly property color watchedAccent: "#c9b563"
    readonly property color watchedBg: "#2d2820"

    property int watchSeconds: 0
    property int trackedSeconds: 0
    property string date: ""
    property string updatedAt: ""
    property var apps: []
    property var domains: []
    property var categories: []
    property var days: []
    property string filterCategory: ""
    property string viewMode: "today"

    property int activeSessionSeconds: 0
    property bool breakOverdue: false
    property bool paused: false
    property string pausedReason: ""
    property string pausedSince: ""
    property bool breaksEnabled: true
    property int breakIntervalSecs: 3600
    property int breaksMinBreakSecs: 300

    property var uncategorizedApps: []
    property var uncategorizedDomains: []
    property bool otherExpanded: false

    property bool settingsOpen: false
    property bool animationsReady: false
    readonly property int todayListMaxHeight: 264

    property bool notificationsEnabled: true
    property bool notificationsBreakOverdue: true
    property bool notificationsBudgetExceeded: true
    property string focusSourceKind: "auto"
    property string settingsRestartHint: ""

    signal breakStartRequested()
    signal breakEndRequested()
    signal statusRefreshRequested()

    implicitWidth: 520
    implicitHeight: col.implicitHeight + 28
    color: "transparent"

    onVisibleChanged: {
        if (visible) {
            content.opacity = 0;
            content.scale = 0.94;
            popup.animationsReady = false;
            enterAnim.start();
            animationsReadyTimer.restart();
        } else {
            popup.animationsReady = false;
        }
    }

    onViewModeChanged: {
        popup.animationsReady = false;
        animationsReadyTimer.restart();
    }

    onSettingsOpenChanged: {
        popup.animationsReady = false;
        animationsReadyTimer.restart();
    }

    onFilterCategoryChanged: {
        popup.animationsReady = false;
        animationsReadyTimer.restart();
    }

    Timer {
        id: animationsReadyTimer
        interval: 220
        onTriggered: popup.animationsReady = true
    }

    function formatDuration(seconds) {
        if (seconds < 60) return String(seconds) + "s";
        var minutes = Math.floor(seconds / 60);
        if (minutes < 60) return String(minutes) + "m";
        var hours = Math.floor(minutes / 60);
        var rest = minutes % 60;
        return rest > 0 ? String(hours) + "h " + String(rest) + "m" : String(hours) + "h";
    }

    function formatUpdatedAt(iso) {
        if (!iso) return "";
        var t = new Date(iso);
        if (isNaN(t.getTime())) return "";
        var hh = String(t.getHours()).padStart(2, "0");
        var mm = String(t.getMinutes()).padStart(2, "0");
        return hh + ":" + mm;
    }

    function sortBySeconds(list) {
        var copy = (list || []).slice();
        copy.sort(function(a, b) { return (b.seconds || 0) - (a.seconds || 0); });
        return copy;
    }

    function maxSeconds(list) {
        var max = 1;
        var items = list || [];
        for (var i = 0; i < items.length; i++) {
            max = Math.max(max, Number(items[i].seconds || 0));
        }
        return max;
    }

    function cleanAppName(id) {
        var value = String(id || "");
        var known = {
            "brave-browser": "Brave",
            "brave": "Brave",
            "com.mitchellh.ghostty": "Ghostty",
            "ghostty": "Ghostty",
            "org.telegram.desktop": "Telegram",
            "telegram-desktop": "Telegram",
            "code": "Code",
            "cursor": "Cursor",
            "discord": "Discord",
            "signal": "Signal",
            "slack": "Slack",
            "kitty": "Kitty",
            "wezterm": "WezTerm",
            "claude": "Claude",
            "codex": "Codex"
        };
        if (known[value]) return known[value];
        var parts = value.split(".");
        var tail = parts.length > 1 ? parts[parts.length - 1] : value;
        tail = tail.replace(/-/g, " ");
        return tail.replace(/\b\w/g, function(ch) { return ch.toUpperCase(); });
    }

    function cleanDomainName(domain) {
        var value = String(domain || "").replace(/^www\./, "");
        var known = {
            "x.com": "X",
            "twitter.com": "X",
            "youtube.com": "YouTube",
            "youtu.be": "YouTube",
            "github.com": "GitHub",
            "web.whatsapp.com": "WhatsApp",
            "chatgpt.com": "ChatGPT",
            "claude.ai": "Claude",
            "gemini.google.com": "Gemini",
            "frame.work": "Framework"
        };
        if (known[value]) return known[value];
        return value;
    }

    function topCategories(n) {
        var cats = (popup.categories || []).slice();
        cats.sort(function(a, b) { return (b.seconds || 0) - (a.seconds || 0); });
        return cats.filter(function(c) { return (c.seconds || 0) > 0; }).slice(0, n);
    }

    function sumSeconds(list) {
        var sum = 0;
        for (var i = 0; i < list.length; i++) sum += Number(list[i].seconds || 0);
        return sum;
    }

    function dayLabel(isoDate, index) {
        if (index === 0) return "Today";
        if (index === 1) return "Yesterday";
        var parts = String(isoDate || "").split("-");
        if (parts.length !== 3) return String(isoDate || "");
        var d = new Date(Number(parts[0]), Number(parts[1]) - 1, Number(parts[2]));
        var weekday = ["Sun","Mon","Tue","Wed","Thu","Fri","Sat"][d.getDay()];
        var dayNum = d.getDate();
        return weekday + " " + dayNum;
    }

    function maxTrackedAcrossDays() {
        var max = 1;
        var ds = popup.days || [];
        for (var i = 0; i < ds.length; i++) {
            max = Math.max(max, Number(ds[i].tracked_seconds || 0));
        }
        return max;
    }

    function trackedApps() {
        var list = (popup.apps || []).filter(function(a) {
            if (!a.watched) return false;
            if (popup.filterCategory && popup.filterCategory !== "") return a.category === popup.filterCategory;
            return true;
        });
        list.sort(function(a, b) { return (b.seconds || 0) - (a.seconds || 0); });
        return list;
    }

    function trackedDomains() {
        var list = (popup.domains || []).filter(function(d) {
            if (!d.watched) return false;
            if (popup.filterCategory && popup.filterCategory !== "") return d.category === popup.filterCategory;
            return true;
        });
        list.sort(function(a, b) { return (b.seconds || 0) - (a.seconds || 0); });
        return list;
    }

    function uncategorizedAppsList() {
        return (popup.uncategorizedApps || []).slice().sort(function(a, b) {
            return (b.seconds || 0) - (a.seconds || 0);
        });
    }

    function uncategorizedDomainsList() {
        return (popup.uncategorizedDomains || []).slice().sort(function(a, b) {
            return (b.seconds || 0) - (a.seconds || 0);
        });
    }

    function categoryColor(name) {
        switch (name) {
            case "coding": return "#7ec8e3";
            case "ai": return "#c39ddb";
            case "design": return "#e3a07e";
            case "productivity": return "#e3d97e";
            case "meeting": return "#7ec39d";
            case "terminal": return "#d8b45c";
            case "chat": return "#9dc3e3";
            case "music": return "#c3e3a0";
            case "video": return "#e37e9d";
            case "scroll": return "#e3957e";
            case "games": return "#b07ee3";
            case "editor": return "#7ec3a0";
            case "email": return "#8ec0d0";
            case "storage": return "#a8b8c8";
            case "news": return "#d0a878";
            case "shopping": return "#d8b89a";
            case "finance": return "#9fd09f";
            case "learning": return "#b89be3";
            case "search": return "#b0b0c8";
            case "reference": return "#c8c8a0";
            case "devops": return "#7ea0e3";
            case "travel": return "#9dd0c8";
            case "food": return "#e3b07e";
            case "sports": return "#e39d7e";
            case "health": return "#a0d09d";
            case "read_later": return "#c8a8d8";
            case "media": return "#e3c0a0";
            default: return popup.accentColor;
        }
    }

    function formatBudget(seconds, budgetSecs) {
        if (!budgetSecs || budgetSecs <= 0) return "";
        return formatDuration(seconds) + " / " + formatDuration(budgetSecs);
    }

    function categoryAlertColor(name, overBudget) {
        return overBudget
            ? Qt.tint(categoryColor(name), Qt.rgba(0.85, 0.48, 0.42, 0.55))
            : categoryColor(name);
    }

    function iconFor(label, category, type) {
        var name = String(label || "").toLowerCase();
        var cat = String(category || "").toLowerCase();
        var t = String(type || "").toLowerCase();

        if (name.indexOf("claude") !== -1) return "\u{F06A9}";
        if (name.indexOf("codex") !== -1) return "\u{F06A9}";
        if (name.indexOf("ghostty") !== -1 || name.indexOf("kitty") !== -1 || name.indexOf("wezterm") !== -1) return "\u{F120}";
        if (name.indexOf("brave") !== -1) return "\u{F268}";
        if (name === "x" || name === "twitter") return "\u{F059F}";
        if (name.indexOf("github") !== -1) return "\u{F02A4}";
        if (name.indexOf("youtube") !== -1) return "\u{F05C3}";
        if (name.indexOf("telegram") !== -1 || name.indexOf("whatsapp") !== -1 || name.indexOf("signal") !== -1) return "\u{F0361}";

        if (cat === "ai") return "\u{F06A9}";
        if (cat === "terminal") return "\u{F120}";
        if (cat === "coding" || cat === "editor") return "\u{F02A4}";
        if (cat === "chat") return "\u{F0361}";
        if (cat === "video") return "\u{F05C3}";
        if (cat === "scroll") return "\u{F059F}";

        if (t === "domain") return "\u{F059F}";
        return "\u{F01C4}";
    }

    function topItems() {
        var out = [];
        var appItems = popup.trackedApps();
        for (var i = 0; i < appItems.length; i++) {
            out.push({
                type: "app",
                label: popup.cleanAppName(appItems[i].id),
                rawLabel: appItems[i].id,
                category: appItems[i].category || "",
                seconds: Number(appItems[i].seconds || 0),
                watched: !!appItems[i].watched
            });
        }
        var domainItems = popup.trackedDomains();
        for (var j = 0; j < domainItems.length; j++) {
            out.push({
                type: "domain",
                label: popup.cleanDomainName(domainItems[j].domain),
                rawLabel: domainItems[j].domain,
                category: domainItems[j].category || "",
                seconds: Number(domainItems[j].seconds || 0),
                watched: !!domainItems[j].watched
            });
        }
        out.sort(function(a, b) { return b.seconds - a.seconds; });
        return out.slice(0, 4);
    }

    function applyBreakSettings(enabled, interval, minBreak) {
        attnSetBreaks.desiredEnabled = enabled;
        attnSetBreaks.desiredInterval = interval;
        attnSetBreaks.desiredMinBreak = minBreak;
        attnSetBreaks.running = true;
    }

    function applyNotificationSettings(enabled, breakOverdue, budgetExceeded) {
        attnSetNotifications.nEnabled = enabled;
        attnSetNotifications.nBreakOverdue = breakOverdue;
        attnSetNotifications.nBudgetExceeded = budgetExceeded;
        attnSetNotifications.running = true;
    }

    function applyFocusSource(kind) {
        attnSetFocusSource.fsKind = kind;
        attnSetFocusSource.running = true;
        popup.settingsRestartHint = "Focus source saved — restart daemon to apply.";
    }

    function requestBreakStart() {
        if (!attnBreakStart.running) {
            attnBreakStart.exec(attnBreakStart.command);
        }
    }

    function requestBreakEnd() {
        if (!attnBreakEnd.running) {
            attnBreakEnd.exec(attnBreakEnd.command);
        }
    }

    Rectangle {
        id: content
        anchors.fill: parent
        radius: 12
        color: popup.bgColor
        border.width: 1
        border.color: popup.accentColor
        opacity: 0
        transformOrigin: Item.Top

        Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
        Behavior on scale { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }

        ParallelAnimation {
            id: enterAnim
            NumberAnimation { target: content; property: "opacity"; to: 1.0; duration: 200 }
            NumberAnimation { target: content; property: "scale"; to: 1.0; duration: 220 }
        }

        ColumnLayout {
            id: col
            anchors.fill: parent
            anchors.margins: 12
            spacing: 12

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                Text {
                    text: "\u{F0210}"
                    color: popup.textColor
                    font.family: popup.fontFamily
                    font.pixelSize: 18
                }
                Text {
                    text: "attn"
                    color: popup.textColor
                    font.family: popup.fontFamily
                    font.pixelSize: 14
                    font.bold: true
                }
                Text {
                    text: popup.date
                    color: popup.textDim
                    font.family: popup.fontFamily
                    font.pixelSize: 11
                    Layout.fillWidth: true
                }
                Text {
                    text: popup.updatedAt ? "↻ " + popup.formatUpdatedAt(popup.updatedAt) : ""
                    color: popup.textDim
                    font.family: popup.fontFamily
                    font.pixelSize: 9
                }

                Rectangle {
                    id: gearButton
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 22
                    radius: 11
                    color: popup.settingsOpen
                        ? popup.watchedBg
                        : (gearMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.15) : "transparent")
                    border.width: 1
                    border.color: popup.settingsOpen ? popup.watchedAccent : Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.5)
                    scale: gearMouse.pressed ? 0.92 : 1.0

                    Behavior on color { ColorAnimation { duration: 160; easing.type: Easing.OutCubic } }
                    Behavior on border.color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: 100; easing.type: Easing.OutCubic } }

                    Text {
                        id: gearGlyph
                        text: "\u{F0493}"
                        color: popup.settingsOpen ? popup.watchedAccent : popup.textDim
                        font.family: popup.fontFamily
                        font.pixelSize: 12
                        anchors.centerIn: parent
                    }
                    MouseArea {
                        id: gearMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            popup.animationsReady = false;
                            animationsReadyTimer.restart();
                            popup.settingsOpen = !popup.settingsOpen;
                        }
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                Row {
                    spacing: 4
                    Layout.alignment: Qt.AlignLeft

                    Repeater {
                        model: [{key: "today", label: "Today"}, {key: "week", label: "Week"}]
                        delegate: Rectangle {
                            id: tabPill
                            required property var modelData
                            readonly property bool active: popup.viewMode === modelData.key
                            width: tabLabel.implicitWidth + 22
                            height: 22
                            radius: 11
                            color: active ? popup.watchedBg : (tabMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.15) : popup.bgSecondary)
                            border.width: 1
                            border.color: active ? popup.watchedAccent : popup.accentColor
                            scale: tabMouse.pressed ? 0.94 : 1.0

                            Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }
                            Behavior on border.color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                            Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

                            Text {
                                id: tabLabel
                                anchors.centerIn: parent
                                text: modelData.label
                                color: active ? popup.watchedAccent : popup.textDim
                                font.family: popup.fontFamily
                                font.pixelSize: 10
                                font.bold: active
                                Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                            }
                            MouseArea {
                                id: tabMouse
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    popup.animationsReady = false;
                                    animationsReadyTimer.restart();
                                    popup.viewMode = modelData.key;
                                }
                            }
                        }
                    }
                }

                Item { Layout.fillWidth: true }

                Rectangle {
                    id: breakChip
                    readonly property bool alert: popup.breakOverdue || popup.paused
                    readonly property int remainingSecs: Math.max(0, popup.breakIntervalSecs - popup.activeSessionSeconds)
                    readonly property int overdueSecs: Math.max(0, popup.activeSessionSeconds - popup.breakIntervalSecs)
                    readonly property real progress: Math.min(1.0, popup.activeSessionSeconds / Math.max(1, popup.breakIntervalSecs))
                    readonly property string chipText: {
                        if (popup.paused && popup.pausedReason === "manual") return "paused";
                        if (popup.paused && popup.pausedReason === "idle") return "on break";
                        if (popup.breakOverdue) return overdueSecs > 0 ? "over by " + popup.formatDuration(overdueSecs) : "take a break";
                        return "break in " + popup.formatDuration(remainingSecs);
                    }

                    Layout.preferredWidth: 152
                    Layout.preferredHeight: 26
                    radius: 13
                    color: breakChip.alert ? popup.watchedBg : popup.bgSecondary
                    border.width: 1
                    border.color: breakChip.alert ? popup.watchedAccent : popup.accentColor
                    clip: true

                    Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    Behavior on border.color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }

                    SequentialAnimation on opacity {
                        running: breakChip.alert
                        loops: Animation.Infinite
                        NumberAnimation { to: 0.65; duration: 1100; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 1.0;  duration: 1100; easing.type: Easing.InOutSine }
                    }

                    // Subtle progress fill across the whole chip background
                    Rectangle {
                        anchors.left: parent.left
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        width: parent.width * breakChip.progress
                        radius: parent.radius
                        color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, breakChip.alert ? 0.18 : 0.10)

                        Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                    }

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 10
                        anchors.rightMargin: 10
                        spacing: 6

                        Text {
                            text: "\u{F0954}"
                            color: breakChip.alert ? popup.watchedAccent : popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 11

                            Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                        }
                        Text {
                            Layout.fillWidth: true
                            text: breakChip.chipText
                            color: breakChip.alert ? popup.watchedAccent : popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                            font.bold: breakChip.alert
                            elide: Text.ElideRight

                            Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                visible: popup.viewMode === "week"

                Repeater {
                    model: popup.days
                    delegate: Item {
                        id: dayItem
                        required property var modelData
                        required property int index
                        readonly property var day: modelData
                        readonly property real dayTotal: Math.max(1, Number(modelData.tracked_seconds || 0))
                        readonly property var topCats: (modelData.categories || []).slice(0, 5)
                        Layout.fillWidth: true
                        Layout.preferredHeight: dayItem.topCats.length > 0 ? 58 : 38

                        Rectangle {
                            anchors.fill: parent
                            radius: 6
                            color: dayHover.containsMouse ? Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.08) : "transparent"
                            Behavior on color { ColorAnimation { duration: 100; easing.type: Easing.OutCubic } }
                        }

                        MouseArea {
                            id: dayHover
                            anchors.fill: parent
                            hoverEnabled: true
                        }

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 6
                            anchors.rightMargin: 6
                            anchors.topMargin: 4
                            anchors.bottomMargin: 4
                            spacing: 4

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8

                                Text {
                                    text: popup.dayLabel(dayItem.day.date, dayItem.index)
                                    color: dayItem.index === 0 ? popup.textColor : popup.textDim
                                    font.family: popup.fontFamily
                                    font.pixelSize: 11
                                    font.bold: dayItem.index === 0
                                    Layout.preferredWidth: 80
                                }
                                Text {
                                    text: popup.formatDuration(Number(dayItem.day.tracked_seconds || 0))
                                    color: popup.textColor
                                    font.family: popup.fontFamily
                                    font.pixelSize: 11
                                    Layout.fillWidth: true
                                }
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 6
                                radius: 3
                                color: Qt.darker(popup.bgSecondary, 1.2)
                                clip: true

                                Row {
                                    id: dayBar
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    height: parent.height
                                    width: parent.width * Number(dayItem.day.tracked_seconds || 0) / Math.max(1, popup.maxTrackedAcrossDays())
                                    spacing: 0


                                    Repeater {
                                        model: dayItem.day.categories || []
                                        delegate: Rectangle {
                                            required property var modelData
                                            width: dayBar.width * (modelData.seconds || 0) / dayItem.dayTotal
                                            height: dayBar.height
                                            color: popup.categoryColor(modelData.name)

                                            Behavior on color { ColorAnimation { duration: 250; easing.type: Easing.OutCubic } }
                                        }
                                    }
                                }
                            }

                            Flow {
                                Layout.fillWidth: true
                                spacing: 10
                                visible: dayItem.topCats.length > 0

                                Repeater {
                                    model: dayItem.topCats
                                    delegate: Row {
                                        required property var modelData
                                        spacing: 4

                                        Rectangle {
                                            width: 5
                                            height: 5
                                            radius: 2.5
                                            color: popup.categoryColor(modelData.name)
                                            anchors.verticalCenter: parent.verticalCenter
                                        }
                                        Text {
                                            text: modelData.name
                                            color: dayItem.index === 0 ? popup.textColor : popup.textDim
                                            font.family: popup.fontFamily
                                            font.pixelSize: 9
                                            anchors.verticalCenter: parent.verticalCenter
                                        }
                                        Text {
                                            text: popup.formatDuration(Number(modelData.seconds || 0))
                                            color: popup.textDim
                                            font.family: popup.fontFamily
                                            font.pixelSize: 9
                                            anchors.verticalCenter: parent.verticalCenter
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Rectangle {
                id: overdueBanner
                Layout.fillWidth: true
                Layout.preferredHeight: 34
                radius: 8
                color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, 0.10)
                border.width: 1
                border.color: popup.watchedAccent
                visible: popup.breakOverdue && !popup.paused

                SequentialAnimation on opacity {
                    running: overdueBanner.visible
                    loops: Animation.Infinite
                    NumberAnimation { to: 0.75; duration: 1400; easing.type: Easing.InOutSine }
                    NumberAnimation { to: 1.0;  duration: 1400; easing.type: Easing.InOutSine }
                }

                RowLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 8
                    spacing: 10

                    Text {
                        text: "\u{F0210}"
                        color: popup.watchedAccent
                        font.family: popup.fontFamily
                        font.pixelSize: 16
                    }

                    Text {
                        Layout.fillWidth: true
                        text: "Time for a break - you've been at the screen for " + popup.formatDuration(popup.activeSessionSeconds) + "."
                        color: popup.textColor
                        font.family: popup.fontFamily
                        font.pixelSize: 11
                        elide: Text.ElideRight
                    }

                    Rectangle {
                        Layout.preferredWidth: bannerBtnLabel.implicitWidth + 18
                        Layout.preferredHeight: 22
                        radius: 11
                        color: bannerBtnMouse.containsMouse ? Qt.lighter(popup.watchedBg, 1.2) : popup.watchedBg
                        border.width: 1
                        border.color: popup.watchedAccent
                        scale: bannerBtnMouse.pressed ? 0.94 : 1.0

                        Behavior on color { ColorAnimation { duration: 160; easing.type: Easing.OutCubic } }
                        Behavior on scale { NumberAnimation { duration: 100; easing.type: Easing.OutCubic } }

                        Text {
                            id: bannerBtnLabel
                            anchors.centerIn: parent
                            text: "Take one"
                            color: popup.watchedAccent
                            font.family: popup.fontFamily
                            font.pixelSize: 10
                            font.bold: true
                        }

                        MouseArea {
                            id: bannerBtnMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: popup.requestBreakStart()
                        }
                    }
                }
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: header.implicitHeight + 24
                radius: 8
                color: popup.bgColor
                visible: popup.viewMode === "today"
                border.width: 1
                border.color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.35)

                ColumnLayout {
                    id: header
                    anchors.fill: parent
                    anchors.margins: 12
                    spacing: 10

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        Text {
                            text: popup.formatDuration(popup.trackedSeconds)
                            color: popup.textColor
                            font.family: popup.fontFamily
                            font.pixelSize: 20
                            font.bold: true
                        }
                        Text {
                            text: "Today's focus"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                            Layout.fillWidth: true
                            Layout.alignment: Qt.AlignBottom
                            bottomPadding: 3
                        }
                    }

                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 8
                        radius: 4
                        color: Qt.darker(popup.bgSecondary, 1.2)
                        clip: true

                        Row {
                            anchors.fill: parent
                            spacing: 0

                            Repeater {
                                model: popup.topCategories(6)
                                delegate: Rectangle {
                                    id: barSegment
                                    required property var modelData
                                    readonly property int segSeconds: modelData.seconds || 0
                                    readonly property int segBudget: modelData.budget_secs || 0
                                    width: parent.width * segSeconds / Math.max(1, popup.sumSeconds(popup.topCategories(6)))
                                    height: parent.height
                                    color: popup.categoryAlertColor(modelData.name, !!modelData.over_budget)
                                    clip: true

                                    Behavior on color { ColorAnimation { duration: 250; easing.type: Easing.OutCubic } }

                                    // Budget threshold stripe — only visible when budget is set and not yet exceeded
                                    Rectangle {
                                        visible: barSegment.segBudget > 0 && !modelData.over_budget && barSegment.segSeconds > 0
                                        width: 1
                                        height: parent.height
                                        opacity: 0.4
                                        color: "#ffffff"
                                        x: Math.min(
                                            barSegment.width * barSegment.segBudget / Math.max(1, barSegment.segSeconds),
                                            barSegment.width - 1
                                        )
                                    }
                                }
                            }
                        }
                    }

                    Flow {
                        Layout.fillWidth: true
                        spacing: 10

                        Repeater {
                            model: popup.topCategories(6)
                            delegate: Row {
                                required property var modelData
                                spacing: 5

                                Rectangle {
                                    width: 6
                                    height: 6
                                    radius: 3
                                    color: popup.categoryAlertColor(modelData.name, !!modelData.over_budget)
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                Text {
                                    text: modelData.name
                                    color: modelData.over_budget ? "#d97b6c" : popup.textColor
                                    font.family: popup.fontFamily
                                    font.pixelSize: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                                Text {
                                    readonly property string budgetStr: popup.formatBudget(modelData.seconds || 0, modelData.budget_secs || 0)
                                    text: budgetStr !== "" ? budgetStr : popup.formatDuration(modelData.seconds || 0)
                                    color: modelData.over_budget ? "#d97b6c" : popup.textDim
                                    font.family: popup.fontFamily
                                    font.pixelSize: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                            }
                        }
                    }
                }
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: 1
                color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.2)
                visible: popup.viewMode === "today"
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.preferredHeight: 30
                spacing: 6
                visible: popup.viewMode === "today"

                Repeater {
                    model: popup.topItems()
                    delegate: Rectangle {
                        id: topPill
                        required property var modelData
                        Layout.fillWidth: true
                        Layout.preferredHeight: 30
                        radius: 15
                        color: pillHover.containsMouse
                            ? Qt.lighter(modelData.watched ? popup.watchedBg : popup.bgSecondary, 1.18)
                            : (modelData.watched ? popup.watchedBg : popup.bgSecondary)
                        border.width: 1
                        border.color: modelData.watched ? popup.watchedAccent : popup.accentColor
                        scale: pillHover.containsMouse ? 1.03 : 1.0

                        Behavior on color { ColorAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        Behavior on scale { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }

                        MouseArea {
                            id: pillHover
                            anchors.fill: parent
                            hoverEnabled: true
                        }

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 10
                            anchors.rightMargin: 10
                            spacing: 6

                            Text {
                                text: popup.iconFor(modelData.label, modelData.category, modelData.type)
                                color: modelData.watched ? popup.watchedAccent : popup.textColor
                                font.family: popup.fontFamily
                                font.pixelSize: 13
                            }
                            Text {
                                text: modelData.label
                                color: popup.textColor
                                font.family: popup.fontFamily
                                font.pixelSize: 11
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Text {
                                text: popup.formatDuration(modelData.seconds || 0)
                                color: popup.textDim
                                font.family: popup.fontFamily
                                font.pixelSize: 9
                            }
                        }
                    }
                }
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: 1
                color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.2)
                visible: popup.viewMode === "today"
            }

            Flow {
                Layout.fillWidth: true
                spacing: 4
                visible: popup.viewMode === "today" && (popup.categories || []).length > 0

                Repeater {
                    model: [{name: "all"}].concat(popup.topCategories(20))
                    delegate: Rectangle {
                        required property var modelData
                        readonly property bool active: (modelData.name === "all" && popup.filterCategory === "") || popup.filterCategory === modelData.name
                        radius: 11
                        height: 22
                        width: chipLabel.implicitWidth + 18
                        color: active
                            ? popup.watchedBg
                            : (chipMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.15) : popup.bgSecondary)
                        border.width: 1
                        border.color: active ? popup.watchedAccent : popup.accentColor
                        scale: chipMouse.pressed ? 0.94 : 1.0

                        Behavior on color { ColorAnimation { duration: 160; easing.type: Easing.OutCubic } }
                        Behavior on border.color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                        Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

                        Text {
                            id: chipLabel
                            anchors.centerIn: parent
                            text: modelData.name
                            color: active ? popup.watchedAccent : popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                            Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
                        }
                        MouseArea {
                            id: chipMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                popup.animationsReady = false;
                                animationsReadyTimer.restart();
                                popup.filterCategory = (modelData.name === "all" ? "" : modelData.name);
                            }
                        }
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignTop
                spacing: 16
                visible: popup.viewMode === "today"

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.preferredWidth: 1
                    Layout.alignment: Qt.AlignTop
                    spacing: 2

                    Text {
                        text: "Apps"
                        color: popup.textColor
                        font.family: popup.fontFamily
                        font.pixelSize: 11
                        font.bold: true
                        visible: popup.trackedApps().length > 0
                        Layout.bottomMargin: 4
                    }
                    ListView {
                        id: appsList
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        interactive: contentHeight > height
                        spacing: 2
                        model: popup.trackedApps()
                        delegate: AttnRow {
                            required property var modelData
                            width: Math.max(0, appsList.width - (appsList.interactive ? 8 : 0))
                            icon: popup.iconFor(modelData.id, modelData.category, "app")
                            label: popup.cleanAppName(modelData.id)
                            category: modelData.category || ""
                            seconds: modelData.seconds || 0
                            watched: !!modelData.watched
                            maxSeconds: popup.maxSeconds(popup.trackedApps())
                            textColor: popup.textColor
                            textDim: popup.textDim
                            accentColor: popup.accentColor
                            bgSecondary: popup.bgSecondary
                            watchedAccent: popup.watchedAccent
                            watchedBg: popup.watchedBg
                            fontFamily: popup.fontFamily
                            animateBar: popup.animationsReady
                        }
                        ScrollBar.vertical: ScrollBar {
                            policy: appsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                            width: 5
                            contentItem: Rectangle {
                                implicitWidth: 5
                                radius: 2
                                color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, 0.65)
                            }
                            background: Rectangle {
                                implicitWidth: 5
                                radius: 2
                                color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.14)
                            }
                        }
                    }
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.preferredWidth: 1
                    Layout.alignment: Qt.AlignTop
                    spacing: 2

                    Text {
                        text: "Domains"
                        color: popup.textColor
                        font.family: popup.fontFamily
                        font.pixelSize: 11
                        font.bold: true
                        visible: popup.trackedDomains().length > 0
                        Layout.bottomMargin: 4
                    }
                    ListView {
                        id: domainsList
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        interactive: contentHeight > height
                        spacing: 2
                        model: popup.trackedDomains()
                        delegate: AttnRow {
                            required property var modelData
                            width: Math.max(0, domainsList.width - (domainsList.interactive ? 8 : 0))
                            icon: popup.iconFor(modelData.domain, modelData.category, "domain")
                            label: popup.cleanDomainName(modelData.domain)
                            category: modelData.category || ""
                            seconds: modelData.seconds || 0
                            watched: !!modelData.watched
                            maxSeconds: popup.maxSeconds(popup.trackedDomains())
                            textColor: popup.textColor
                            textDim: popup.textDim
                            accentColor: popup.accentColor
                            bgSecondary: popup.bgSecondary
                            watchedAccent: popup.watchedAccent
                            watchedBg: popup.watchedBg
                            fontFamily: popup.fontFamily
                            animateBar: popup.animationsReady
                        }
                        ScrollBar.vertical: ScrollBar {
                            policy: domainsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                            width: 5
                            contentItem: Rectangle {
                                implicitWidth: 5
                                radius: 2
                                color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, 0.65)
                            }
                            background: Rectangle {
                                implicitWidth: 5
                                radius: 2
                                color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.14)
                            }
                        }
                    }
                }
            }
        }

        // Other (uncategorized) drawer — shown only when there is data.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 4
            visible: popup.viewMode === "today" && (
                popup.uncategorizedAppsList().length > 0 ||
                popup.uncategorizedDomainsList().length > 0
            )

            // Divider
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: 1
                color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.2)
            }

            // Header row — clickable chevron toggle
            RowLayout {
                Layout.fillWidth: true
                spacing: 6

                Text {
                    text: popup.otherExpanded ? "\u{F0140}" : "\u{F0142}"
                    color: popup.textDim
                    font.family: popup.fontFamily
                    font.pixelSize: 11
                }
                Text {
                    text: "Other"
                    color: popup.textColor
                    font.family: popup.fontFamily
                    font.pixelSize: 11
                    font.bold: true
                    Layout.fillWidth: true
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: popup.otherExpanded = !popup.otherExpanded
                }
            }

            // Expanded content
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 10
                visible: popup.otherExpanded

                RowLayout {
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignTop
                    spacing: 16

                    // Uncategorized apps column
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        Layout.alignment: Qt.AlignTop
                        spacing: 2
                        visible: popup.uncategorizedAppsList().length > 0

                        Text {
                            text: "Apps"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 10
                            Layout.bottomMargin: 2
                        }

                        ListView {
                            id: otherAppsList
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            interactive: contentHeight > height
                            spacing: 2
                            model: popup.uncategorizedAppsList()
                            delegate: Item {
                                required property var modelData
                                width: Math.max(0, otherAppsList.width - (otherAppsList.interactive ? 8 : 0))
                                height: 28

                                readonly property bool isSynthetic: modelData.id === "(below threshold)"

                                AttnRow {
                                    width: parent.isSynthetic ? parent.width : parent.width - 22
                                    height: parent.height
                                    icon: "\u{F01C4}"
                                    label: popup.cleanAppName(modelData.id)
                                    category: ""
                                    seconds: modelData.seconds || 0
                                    watched: false
                                    maxSeconds: popup.maxSeconds(popup.uncategorizedAppsList())
                                    textColor: popup.textColor
                                    textDim: popup.textDim
                                    accentColor: popup.accentColor
                                    bgSecondary: popup.bgSecondary
                                    watchedAccent: popup.watchedAccent
                                    watchedBg: popup.watchedBg
                                    fontFamily: popup.fontFamily
                                    animateBar: popup.animationsReady
                                }

                                Rectangle {
                                    visible: !parent.isSynthetic
                                    anchors.right: parent.right
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 18
                                    height: 18
                                    radius: 9
                                    color: addAppMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.3) : popup.bgSecondary
                                    border.width: 1
                                    border.color: popup.accentColor

                                    Behavior on color { ColorAnimation { duration: 120 } }

                                    Text {
                                        anchors.centerIn: parent
                                        text: "+"
                                        color: popup.textDim
                                        font.family: popup.fontFamily
                                        font.pixelSize: 13
                                    }

                                    MouseArea {
                                        id: addAppMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: {
                                            categoryPickerKind = "app";
                                            categoryPickerItemId = parent.parent.modelData.id;
                                            categoryPickerMenu.popup();
                                        }
                                    }
                                }
                            }

                            ScrollBar.vertical: ScrollBar {
                                policy: otherAppsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                                width: 5
                                contentItem: Rectangle {
                                    implicitWidth: 5
                                    radius: 2
                                    color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, 0.65)
                                }
                                background: Rectangle {
                                    implicitWidth: 5
                                    radius: 2
                                    color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.14)
                                }
                            }
                        }
                    }

                    // Uncategorized domains column
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        Layout.alignment: Qt.AlignTop
                        spacing: 2
                        visible: popup.uncategorizedDomainsList().length > 0

                        Text {
                            text: "Domains"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 10
                            Layout.bottomMargin: 2
                        }

                        ListView {
                            id: otherDomainsList
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            interactive: contentHeight > height
                            spacing: 2
                            model: popup.uncategorizedDomainsList()
                            delegate: Item {
                                required property var modelData
                                width: Math.max(0, otherDomainsList.width - (otherDomainsList.interactive ? 8 : 0))
                                height: 28

                                readonly property bool isSynthetic: modelData.domain === "(below threshold)"

                                AttnRow {
                                    width: parent.isSynthetic ? parent.width : parent.width - 22
                                    height: parent.height
                                    icon: "\u{F059F}"
                                    label: popup.cleanDomainName(modelData.domain)
                                    category: ""
                                    seconds: modelData.seconds || 0
                                    watched: false
                                    maxSeconds: popup.maxSeconds(popup.uncategorizedDomainsList())
                                    textColor: popup.textColor
                                    textDim: popup.textDim
                                    accentColor: popup.accentColor
                                    bgSecondary: popup.bgSecondary
                                    watchedAccent: popup.watchedAccent
                                    watchedBg: popup.watchedBg
                                    fontFamily: popup.fontFamily
                                    animateBar: popup.animationsReady
                                }

                                Rectangle {
                                    visible: !parent.isSynthetic
                                    anchors.right: parent.right
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 18
                                    height: 18
                                    radius: 9
                                    color: addDomainMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.3) : popup.bgSecondary
                                    border.width: 1
                                    border.color: popup.accentColor

                                    Behavior on color { ColorAnimation { duration: 120 } }

                                    Text {
                                        anchors.centerIn: parent
                                        text: "+"
                                        color: popup.textDim
                                        font.family: popup.fontFamily
                                        font.pixelSize: 13
                                    }

                                    MouseArea {
                                        id: addDomainMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: {
                                            categoryPickerKind = "domain";
                                            categoryPickerItemId = parent.parent.modelData.domain;
                                            categoryPickerMenu.popup();
                                        }
                                    }
                                }
                            }

                            ScrollBar.vertical: ScrollBar {
                                policy: otherDomainsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                                width: 5
                                contentItem: Rectangle {
                                    implicitWidth: 5
                                    radius: 2
                                    color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, 0.65)
                                }
                                background: Rectangle {
                                    implicitWidth: 5
                                    radius: 2
                                    color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.14)
                                }
                            }
                        }
                    }
                }
            }
        }

        // Category picker state (shared by app and domain "+" buttons)
        property string categoryPickerKind: ""
        property string categoryPickerItemId: ""

        Menu {
            id: categoryPickerMenu
            title: "Add to category"

            Instantiator {
                model: popup.categories
                delegate: MenuItem {
                    required property var modelData
                    text: modelData.name || ""
                    onTriggered: {
                        attnCategorize.categorizeKind = popup.categoryPickerKind;
                        attnCategorize.categorizeId = popup.categoryPickerItemId;
                        attnCategorize.categorizeCategory = modelData.name;
                        attnCategorize.running = true;
                    }
                }
                onObjectAdded: (index, object) => categoryPickerMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => categoryPickerMenu.removeItem(object)
            }
        }

        Process {
            id: attnCategorize
            property string categorizeKind: ""
            property string categorizeId: ""
            property string categorizeCategory: ""
            command: ["attn", "categorize",
                "--kind=" + categorizeKind,
                "--id=" + categorizeId,
                "--category=" + categorizeCategory]
            onExited: popup.statusRefreshRequested()
        }

        Process {
            id: attnBreakStart
            command: ["attn", "break-start"]
            onExited: popup.statusRefreshRequested()
        }
        Process {
            id: attnBreakEnd
            command: ["attn", "break-end"]
            onExited: popup.statusRefreshRequested()
        }
        Process {
            id: attnSetBreaks
            property bool desiredEnabled: popup.breaksEnabled
            property int desiredInterval: popup.breakIntervalSecs
            property int desiredMinBreak: popup.breaksMinBreakSecs
            command: ["attn", "set-breaks",
                "--enabled=" + (desiredEnabled ? "true" : "false"),
                "--interval=" + String(desiredInterval),
                "--min-break=" + String(desiredMinBreak)]
            onExited: popup.statusRefreshRequested()
        }

        Process {
            id: attnSetNotifications
            property bool nEnabled: popup.notificationsEnabled
            property bool nBreakOverdue: popup.notificationsBreakOverdue
            property bool nBudgetExceeded: popup.notificationsBudgetExceeded
            command: ["attn", "set-notifications",
                "--enabled=" + (nEnabled ? "true" : "false"),
                "--break-overdue=" + (nBreakOverdue ? "true" : "false"),
                "--budget-exceeded=" + (nBudgetExceeded ? "true" : "false")]
            onExited: popup.statusRefreshRequested()
        }

        Process {
            id: attnSetFocusSource
            property string fsKind: popup.focusSourceKind
            command: ["attn", "set-focus-source", "--kind=" + fsKind]
            onExited: popup.statusRefreshRequested()
        }

        Process {
            id: attnSetBudget
            property string budgetCategory: ""
            property int budgetSecs: 0
            command: ["attn", "set-budget",
                "--category=" + budgetCategory,
                "--secs=" + String(budgetSecs)]
            onExited: popup.statusRefreshRequested()
        }

        Rectangle {
            id: settingsOverlay
            anchors.fill: parent
            anchors.margins: 1
            radius: 11
            color: Qt.rgba(popup.bgColor.r, popup.bgColor.g, popup.bgColor.b, 0.96)
            visible: popup.settingsOpen
            opacity: visible ? 1.0 : 0.0
            Behavior on opacity { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }

            MouseArea { anchors.fill: parent }

            // Header pinned at top
            RowLayout {
                id: settingsHeader
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.topMargin: 14
                anchors.leftMargin: parent.width * 0.11
                anchors.rightMargin: parent.width * 0.11
                opacity: settingsOverlay.visible ? 1.0 : 0.0
                scale: settingsOverlay.visible ? 1.0 : 0.94

                Behavior on opacity { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
                Behavior on scale { NumberAnimation { duration: 300; easing.type: Easing.OutBack } }

                Text {
                    text: "\u{F0493}  Settings"
                    color: popup.textColor
                    font.family: popup.fontFamily
                    font.pixelSize: 14
                    font.bold: true
                    Layout.fillWidth: true
                }
                Rectangle {
                    Layout.preferredWidth: 24
                    Layout.preferredHeight: 24
                    radius: 12
                    color: closeMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.2) : "transparent"
                    border.width: 1
                    border.color: popup.accentColor

                    Behavior on color { ColorAnimation { duration: 160 } }

                    Text {
                        anchors.centerIn: parent
                        text: "×"
                        color: popup.textDim
                        font.family: popup.fontFamily
                        font.pixelSize: 13
                    }
                    MouseArea {
                        id: closeMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            popup.settingsOpen = false;
                            popup.settingsRestartHint = "";
                        }
                    }
                }
            }

            // Scrollable settings content
            Flickable {
                id: settingsFlickable
                anchors.top: settingsHeader.bottom
                anchors.topMargin: 12
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 14
                anchors.left: parent.left
                anchors.right: parent.right
                clip: true
                contentWidth: width
                contentHeight: settingsContent.implicitHeight
                boundsBehavior: Flickable.StopAtBounds

                ScrollBar.vertical: ScrollBar {
                    policy: settingsFlickable.contentHeight > settingsFlickable.height
                            ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                    width: 5
                    contentItem: Rectangle {
                        implicitWidth: 5
                        radius: 2
                        color: Qt.rgba(popup.watchedAccent.r, popup.watchedAccent.g, popup.watchedAccent.b, 0.65)
                    }
                    background: Rectangle {
                        implicitWidth: 5
                        radius: 2
                        color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.14)
                    }
                }

                ColumnLayout {
                    id: settingsContent
                    width: settingsFlickable.width * 0.78
                    anchors.horizontalCenter: parent.horizontalCenter
                    spacing: 18
                    opacity: settingsOverlay.visible ? 1.0 : 0.0
                    scale: settingsOverlay.visible ? 1.0 : 0.94

                    Behavior on opacity { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: 300; easing.type: Easing.OutBack } }

                    // ── Break reminder ────────────────────────────────────────
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 10

                        RowLayout {
                            Layout.fillWidth: true

                            Text {
                                text: "Break reminder"
                                color: popup.textColor
                                font.family: popup.fontFamily
                                font.pixelSize: 11
                                font.bold: true
                                Layout.fillWidth: true
                            }
                            Rectangle {
                                id: enabledSwitch
                                Layout.preferredWidth: 42
                                Layout.preferredHeight: 22
                                radius: 11
                                color: popup.breaksEnabled ? popup.watchedAccent : Qt.darker(popup.bgSecondary, 1.4)
                                border.width: 1
                                border.color: popup.breaksEnabled ? popup.watchedAccent : popup.accentColor

                                Behavior on color { ColorAnimation { duration: 180; easing.type: Easing.OutCubic } }

                                Rectangle {
                                    width: 16
                                    height: 16
                                    radius: 8
                                    color: popup.bgColor
                                    anchors.verticalCenter: parent.verticalCenter
                                    x: popup.breaksEnabled ? parent.width - width - 3 : 3
                                    Behavior on x { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                                }

                                MouseArea {
                                    anchors.fill: parent
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: popup.applyBreakSettings(!popup.breaksEnabled, popup.breakIntervalSecs, popup.breaksMinBreakSecs)
                                }
                            }
                        }

                        Text {
                            text: "Prompt after"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                            opacity: popup.breaksEnabled ? 1.0 : 0.5
                            Behavior on opacity { NumberAnimation { duration: 200 } }
                        }
                        Row {
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: popup.breaksEnabled ? 1.0 : 0.5
                            Behavior on opacity { NumberAnimation { duration: 200 } }

                            Repeater {
                                model: [
                                    {label: "30m",  secs: 1800},
                                    {label: "1h",   secs: 3600},
                                    {label: "90m",  secs: 5400},
                                    {label: "2h",   secs: 7200},
                                    {label: "3h",   secs: 10800},
                                ]
                                delegate: Rectangle {
                                    required property var modelData
                                    readonly property bool active: popup.breakIntervalSecs === modelData.secs
                                    width: intervalLabel.implicitWidth + 18
                                    height: 24
                                    radius: 12
                                    color: active
                                        ? popup.watchedBg
                                        : (intervalMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.15) : popup.bgSecondary)
                                    border.width: 1
                                    border.color: active ? popup.watchedAccent : popup.accentColor
                                    scale: intervalMouse.pressed ? 0.94 : 1.0

                                    Behavior on color { ColorAnimation { duration: 160; easing.type: Easing.OutCubic } }
                                    Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

                                    Text {
                                        id: intervalLabel
                                        anchors.centerIn: parent
                                        text: modelData.label
                                        color: active ? popup.watchedAccent : popup.textDim
                                        font.family: popup.fontFamily
                                        font.pixelSize: 10
                                        font.bold: active
                                    }
                                    MouseArea {
                                        id: intervalMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: popup.applyBreakSettings(popup.breaksEnabled, modelData.secs, popup.breaksMinBreakSecs)
                                    }
                                }
                            }
                        }

                        Text {
                            text: "Idle is a break after"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                            opacity: popup.breaksEnabled ? 1.0 : 0.5
                            Behavior on opacity { NumberAnimation { duration: 200 } }
                        }
                        Row {
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: popup.breaksEnabled ? 1.0 : 0.5
                            Behavior on opacity { NumberAnimation { duration: 200 } }

                            Repeater {
                                model: [
                                    {label: "2m",  secs: 120},
                                    {label: "5m",  secs: 300},
                                    {label: "10m", secs: 600},
                                    {label: "15m", secs: 900},
                                ]
                                delegate: Rectangle {
                                    required property var modelData
                                    readonly property bool active: popup.breaksMinBreakSecs === modelData.secs
                                    width: minBreakLabel.implicitWidth + 18
                                    height: 24
                                    radius: 12
                                    color: active
                                        ? popup.watchedBg
                                        : (minBreakMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.15) : popup.bgSecondary)
                                    border.width: 1
                                    border.color: active ? popup.watchedAccent : popup.accentColor
                                    scale: minBreakMouse.pressed ? 0.94 : 1.0

                                    Behavior on color { ColorAnimation { duration: 160; easing.type: Easing.OutCubic } }
                                    Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

                                    Text {
                                        id: minBreakLabel
                                        anchors.centerIn: parent
                                        text: modelData.label
                                        color: active ? popup.watchedAccent : popup.textDim
                                        font.family: popup.fontFamily
                                        font.pixelSize: 10
                                        font.bold: active
                                    }
                                    MouseArea {
                                        id: minBreakMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: popup.applyBreakSettings(popup.breaksEnabled, popup.breakIntervalSecs, modelData.secs)
                                    }
                                }
                            }
                        }
                    }

                    // Divider
                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.2) }

                    // ── Budgets ───────────────────────────────────────────────
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        visible: (popup.categories || []).length > 0

                        Text {
                            text: "Budgets"
                            color: popup.textColor
                            font.family: popup.fontFamily
                            font.pixelSize: 11
                            font.bold: true
                        }
                        Text {
                            text: "Minutes per day (0 = no limit)"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                        }

                        Repeater {
                            model: popup.categories || []
                            delegate: RowLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 8

                                // Colour dot
                                Rectangle {
                                    width: 7; height: 7; radius: 3.5
                                    color: popup.categoryColor(modelData.name)
                                    Layout.alignment: Qt.AlignVCenter
                                }

                                Text {
                                    text: modelData.name
                                    color: popup.textColor
                                    font.family: popup.fontFamily
                                    font.pixelSize: 10
                                    Layout.fillWidth: true
                                }

                                Rectangle {
                                    Layout.preferredWidth: 52
                                    Layout.preferredHeight: 22
                                    radius: 4
                                    color: Qt.darker(popup.bgSecondary, 1.3)
                                    border.width: 1
                                    border.color: popup.accentColor

                                    TextInput {
                                        id: budgetInput
                                        anchors.fill: parent
                                        anchors.leftMargin: 6
                                        anchors.rightMargin: 6
                                        verticalAlignment: TextInput.AlignVCenter
                                        color: popup.textColor
                                        font.family: popup.fontFamily
                                        font.pixelSize: 10
                                        inputMethodHints: Qt.ImhDigitsOnly
                                        // Pre-fill from current budget (budget_secs / 60, rounded)
                                        text: (modelData.budget_secs && modelData.budget_secs > 0)
                                              ? String(Math.round(modelData.budget_secs / 60)) : ""
                                        selectByMouse: true
                                        validator: IntValidator { bottom: 0; top: 9999 }
                                    }
                                }

                                Rectangle {
                                    Layout.preferredWidth: budgetSaveLabel.implicitWidth + 14
                                    Layout.preferredHeight: 22
                                    radius: 11
                                    color: budgetSaveMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.2) : popup.bgSecondary
                                    border.width: 1
                                    border.color: popup.accentColor

                                    Behavior on color { ColorAnimation { duration: 120 } }

                                    Text {
                                        id: budgetSaveLabel
                                        anchors.centerIn: parent
                                        text: "Save"
                                        color: popup.textDim
                                        font.family: popup.fontFamily
                                        font.pixelSize: 9
                                    }
                                    MouseArea {
                                        id: budgetSaveMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: {
                                            var mins = parseInt(budgetInput.text) || 0;
                                            attnSetBudget.budgetCategory = modelData.name;
                                            attnSetBudget.budgetSecs = mins * 60;
                                            attnSetBudget.running = true;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Divider
                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.2) }

                    // ── Notifications ─────────────────────────────────────────
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        Text {
                            text: "Notifications"
                            color: popup.textColor
                            font.family: popup.fontFamily
                            font.pixelSize: 11
                            font.bold: true
                        }

                        Repeater {
                            model: [
                                { label: "Enable notifications",  field: "enabled" },
                                { label: "Break-overdue alerts",  field: "breakOverdue" },
                                { label: "Budget-exceeded alerts", field: "budgetExceeded" },
                            ]
                            delegate: RowLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: 10

                                readonly property bool fieldValue: {
                                    switch (modelData.field) {
                                        case "enabled":       return popup.notificationsEnabled;
                                        case "breakOverdue":  return popup.notificationsBreakOverdue;
                                        case "budgetExceeded":return popup.notificationsBudgetExceeded;
                                        default: return false;
                                    }
                                }

                                Text {
                                    text: modelData.label
                                    color: popup.textColor
                                    font.family: popup.fontFamily
                                    font.pixelSize: 10
                                    Layout.fillWidth: true
                                }

                                Rectangle {
                                    Layout.preferredWidth: 42
                                    Layout.preferredHeight: 22
                                    radius: 11
                                    color: fieldValue ? popup.watchedAccent : Qt.darker(popup.bgSecondary, 1.4)
                                    border.width: 1
                                    border.color: fieldValue ? popup.watchedAccent : popup.accentColor

                                    Behavior on color { ColorAnimation { duration: 180; easing.type: Easing.OutCubic } }

                                    Rectangle {
                                        width: 16; height: 16; radius: 8
                                        color: popup.bgColor
                                        anchors.verticalCenter: parent.verticalCenter
                                        x: fieldValue ? parent.width - width - 3 : 3
                                        Behavior on x { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                                    }

                                    MouseArea {
                                        anchors.fill: parent
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: {
                                            var e = popup.notificationsEnabled;
                                            var b = popup.notificationsBreakOverdue;
                                            var u = popup.notificationsBudgetExceeded;
                                            switch (modelData.field) {
                                                case "enabled":        e = !e; break;
                                                case "breakOverdue":   b = !b; break;
                                                case "budgetExceeded": u = !u; break;
                                            }
                                            popup.applyNotificationSettings(e, b, u);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Divider
                    Rectangle { Layout.fillWidth: true; height: 1; color: Qt.rgba(popup.accentColor.r, popup.accentColor.g, popup.accentColor.b, 0.2) }

                    // ── Compositor / Focus source ─────────────────────────────
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        Text {
                            text: "Compositor"
                            color: popup.textColor
                            font.family: popup.fontFamily
                            font.pixelSize: 11
                            font.bold: true
                        }
                        Text {
                            text: "Focus source (requires daemon restart)"
                            color: popup.textDim
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                        }

                        Row {
                            Layout.fillWidth: true
                            spacing: 6

                            Repeater {
                                model: ["auto", "niri", "hyprland", "river", "sway"]
                                delegate: Rectangle {
                                    required property var modelData
                                    readonly property bool active: popup.focusSourceKind === modelData
                                    width: fsLabel.implicitWidth + 14
                                    height: 24
                                    radius: 12
                                    color: active
                                        ? popup.watchedBg
                                        : (fsMouse.containsMouse ? Qt.lighter(popup.bgSecondary, 1.15) : popup.bgSecondary)
                                    border.width: 1
                                    border.color: active ? popup.watchedAccent : popup.accentColor
                                    scale: fsMouse.pressed ? 0.94 : 1.0

                                    Behavior on color { ColorAnimation { duration: 160; easing.type: Easing.OutCubic } }
                                    Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

                                    Text {
                                        id: fsLabel
                                        anchors.centerIn: parent
                                        text: modelData
                                        color: active ? popup.watchedAccent : popup.textDim
                                        font.family: popup.fontFamily
                                        font.pixelSize: 10
                                        font.bold: active
                                    }
                                    MouseArea {
                                        id: fsMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: popup.applyFocusSource(modelData)
                                    }
                                }
                            }
                        }

                        Text {
                            visible: popup.settingsRestartHint !== ""
                            text: popup.settingsRestartHint
                            color: popup.watchedAccent
                            font.family: popup.fontFamily
                            font.pixelSize: 9
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }

                    // Footer hint
                    Text {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        text: "Edit ~/.config/attn/config.toml for watch lists.\nDaemon auto-reloads on save."
                        color: popup.textDim
                        font.family: popup.fontFamily
                        font.pixelSize: 9
                        wrapMode: Text.WordWrap
                        bottomPadding: 4
                    }
                }
            }
        }

        Rectangle {
            id: breakOverlay
            anchors.fill: parent
            anchors.margins: 1
            radius: 11
            color: Qt.rgba(popup.bgColor.r, popup.bgColor.g, popup.bgColor.b, 0.94)
            visible: popup.paused
            opacity: visible ? 1.0 : 0.0
            Behavior on opacity { NumberAnimation { duration: 240; easing.type: Easing.OutCubic } }

            MouseArea { anchors.fill: parent } // swallow clicks behind

            ColumnLayout {
                id: overlayContent
                anchors.centerIn: parent
                spacing: 18
                width: parent.width * 0.7
                opacity: breakOverlay.visible ? 1.0 : 0.0
                scale: breakOverlay.visible ? 1.0 : 0.92

                Behavior on opacity { NumberAnimation { duration: 280; easing.type: Easing.OutCubic } }
                Behavior on scale { NumberAnimation { duration: 320; easing.type: Easing.OutBack } }

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    text: "\u{F0210}"
                    color: popup.watchedAccent
                    font.family: popup.fontFamily
                    font.pixelSize: 36

                    SequentialAnimation on scale {
                        running: breakOverlay.visible && !popup.paused
                        loops: Animation.Infinite
                        NumberAnimation { to: 1.08; duration: 1400; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 1.0;  duration: 1400; easing.type: Easing.InOutSine }
                    }
                }

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                    color: popup.textColor
                    font.family: popup.fontFamily
                    font.pixelSize: 16
                    font.bold: true
                    text: {
                        if (popup.paused && popup.pausedReason === "manual") return "Tracking paused";
                        if (popup.paused && popup.pausedReason === "idle")   return "You're on a break";
                        return "Time for a break";
                    }
                }

                Text {
                    Layout.alignment: Qt.AlignHCenter
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                    color: popup.textDim
                    font.family: popup.fontFamily
                    font.pixelSize: 11
                    text: {
                        if (popup.paused && popup.pausedReason === "manual")
                            return "Tracking is off. Press resume when you're back at the keyboard.";
                        if (popup.paused && popup.pausedReason === "idle")
                            return "Auto-paused after 5 minutes idle. Move the mouse to resume.";
                        return "You've been at the screen for " + popup.formatDuration(popup.activeSessionSeconds) + " straight. Step away for a few minutes.";
                    }
                }

                Rectangle {
                    id: overlayButton
                    Layout.alignment: Qt.AlignHCenter
                    Layout.preferredWidth: 168
                    Layout.preferredHeight: 34
                    radius: 17
                    color: overlayBtnMouse.containsMouse ? Qt.lighter(popup.watchedBg, 1.2) : popup.watchedBg
                    border.width: 1
                    border.color: popup.watchedAccent
                    visible: !(popup.paused && popup.pausedReason === "idle")
                    scale: overlayBtnMouse.pressed ? 0.96 : 1.0

                    Behavior on color { ColorAnimation { duration: 180; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutCubic } }

                    Text {
                        anchors.centerIn: parent
                        text: popup.paused ? "Resume tracking" : "I'll take one now"
                        color: popup.watchedAccent
                        font.family: popup.fontFamily
                        font.pixelSize: 11
                        font.bold: true
                    }

                    MouseArea {
                        id: overlayBtnMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            if (popup.paused) {
                                popup.requestBreakEnd();
                            } else {
                                popup.requestBreakStart();
                            }
                        }
                    }
                }
            }
        }
    }
}
