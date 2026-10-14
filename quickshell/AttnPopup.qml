pragma ComponentBehavior: Bound

import Quickshell
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls.Basic

PopupWindow {
    id: popup

    required property AttnStatus status

    // Every colour has a default, so the popup works without a host theme.
    // A host can override any of them.
    property color bgColor: Qt.rgba(0x16 / 255, 0x16 / 255, 0x1c / 255, 0.62) // translucent, for compositor blur
    property color washColor: Qt.rgba(1, 1, 1, 0.07) // a raised surface inside the popup
    property color textColor: "#d4d4dc"
    property color textDim: Qt.rgba(textColor.r, textColor.g, textColor.b, 0.62)
    property color borderColor: "#7c7c88" // hairline, dividers, unwatched rows
    property color attnColor: "#c9b563"
    property color attnBg: "#2d2820"
    property color accentColor: attnColor // selected chips, toggles that are on
    property color accentText: "#16161c" // text and knobs on an accent fill
    property string fontFamily: "JetBrainsMono Nerd Font"
    readonly property color overBudgetColor: "#d26a6a"
    readonly property color hoverColor: lift(washColor, 0.03)

    property string filterCategory: ""
    property string viewMode: "today"
    property bool otherExpanded: false

    property bool settingsOpen: false
    property bool animationsReady: false
    readonly property int todayListMaxHeight: 264

    // Category picker state (shared by app and domain "+" buttons)
    property string categoryPickerKind: ""
    property string categoryPickerItemId: ""

    implicitWidth: 520
    implicitHeight: surface.implicitHeight
    color: "transparent"

    onVisibleChanged: {
        if (visible) {
            surface.enter();
            popup.animationsReady = false;
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

    // Hover adds light to a wash; Qt.lighter does nothing to white.
    function lift(c, amount) {
        return Qt.rgba(c.r, c.g, c.b, Math.min(1, c.a + amount));
    }

    function alpha(c, a) {
        return Qt.rgba(c.r, c.g, c.b, a);
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
        var cats = (popup.status.categories || []).slice();
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
        var ds = popup.status.days || [];
        for (var i = 0; i < ds.length; i++) {
            max = Math.max(max, Number(ds[i].tracked_seconds || 0));
        }
        return max;
    }

    function trackedApps() {
        var list = (popup.status.apps || []).filter(function(a) {
            if (!a.watched) return false;
            if (popup.filterCategory && popup.filterCategory !== "") return a.category === popup.filterCategory;
            return true;
        });
        list.sort(function(a, b) { return (b.seconds || 0) - (a.seconds || 0); });
        return list;
    }

    function trackedDomains() {
        var list = (popup.status.domains || []).filter(function(d) {
            if (!d.watched) return false;
            if (popup.filterCategory && popup.filterCategory !== "") return d.category === popup.filterCategory;
            return true;
        });
        list.sort(function(a, b) { return (b.seconds || 0) - (a.seconds || 0); });
        return list;
    }

    function uncategorizedAppsList() {
        return (popup.status.uncategorizedApps || []).slice().sort(function(a, b) {
            return (b.seconds || 0) - (a.seconds || 0);
        });
    }

    function uncategorizedDomainsList() {
        return (popup.status.uncategorizedDomains || []).slice().sort(function(a, b) {
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
            default: return popup.borderColor;
        }
    }

    function formatBudget(seconds, budgetSecs) {
        if (!budgetSecs || budgetSecs <= 0) return "";
        return formatDuration(seconds) + " / " + formatDuration(budgetSecs);
    }

    function categoryAlertColor(name, overBudget) {
        return overBudget
            ? Qt.tint(categoryColor(name), popup.alpha(popup.overBudgetColor, 0.55))
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

    // ---- controls ----------------------------------------------------------
    // Private to this file. They read the colours above through `popup`,
    // which the Bound pragma lets an inline component see.

    component SText: Text {
        font.family: popup.fontFamily
        font.pixelSize: 12
        color: popup.textColor
    }

    // A small dim label: list headings, row labels.
    component Caption: SText {
        font.pixelSize: 11
        color: popup.textDim
        elide: Text.ElideRight
    }

    component Divider: Rectangle {
        width: parent ? parent.width : 0
        implicitHeight: 1
        color: popup.alpha(popup.borderColor, 0.3)
    }

    // Padding 0: the Basic style's 2px padding leaves a 3px bar no room for
    // its thumb.
    component ScrollRail: ScrollBar {
        width: 3
        padding: 0

        contentItem: Rectangle {
            implicitWidth: 3
            radius: 1.5
            color: popup.alpha(popup.textDim, 0.4)
        }
    }

    // A bare glyph button: no container at rest, a wash on hover.
    component IconButton: Rectangle {
        id: iconBtn

        property string icon
        property color iconColor: popup.textColor
        property int size: 28
        signal clicked

        implicitWidth: size
        implicitHeight: size
        radius: 6
        color: iconArea.containsMouse ? popup.hoverColor : "transparent"

        Behavior on color { ColorAnimation { duration: 100; easing.type: Easing.OutCubic } }

        SText {
            anchors.centerIn: parent
            text: iconBtn.icon
            font.pixelSize: 14
            color: iconBtn.iconColor
        }

        MouseArea {
            id: iconArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: iconBtn.clicked()
        }
    }

    // A button on the wash. `selected` is the accent fill of a picked option.
    component Chip: Rectangle {
        id: chipRoot

        property string text
        property bool selected
        property color textColor: popup.textColor
        readonly property bool hovered: chipArea.containsMouse
        signal clicked

        implicitWidth: chipLabel.implicitWidth + 20
        implicitHeight: 28
        radius: height >= 34 ? 8 : 6
        color: selected ? (hovered ? Qt.lighter(popup.accentColor, 1.1) : popup.accentColor)
             : hovered ? popup.lift(popup.washColor, 0.05) : popup.washColor

        Behavior on color { ColorAnimation { duration: 100; easing.type: Easing.OutCubic } }

        SText {
            id: chipLabel
            anchors.centerIn: parent
            // Elides rather than spilling out of a chip narrower than its text.
            width: Math.min(implicitWidth, chipRoot.width - 12)
            text: chipRoot.text
            font.pixelSize: 11
            font.bold: chipRoot.selected
            color: chipRoot.textColor
            elide: Text.ElideRight
        }

        MouseArea {
            id: chipArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: chipRoot.clicked()
        }
    }

    // One-of-n chips that share the row width. `options` holds plain values,
    // or objects { value, label }.
    component Segmented: RowLayout {
        id: seg

        property var options: []
        property var current
        property int chipHeight: 28
        signal picked(var value)

        spacing: 6

        Repeater {
            model: seg.options

            Chip {
                required property var modelData
                readonly property var value: typeof modelData === "object" ? modelData.value : modelData

                Layout.fillWidth: true
                Layout.preferredWidth: 1 // equal shares, not shares by label length
                implicitHeight: seg.chipHeight
                text: typeof modelData === "object" ? modelData.label : String(modelData)
                selected: value === seg.current
                textColor: selected ? popup.accentText : hovered ? popup.textColor : popup.textDim
                onClicked: seg.picked(value)
            }
        }
    }

    // On/off switch. It reports the click and leaves `checked` to the
    // caller's binding.
    component Toggle: Rectangle {
        id: toggleRoot

        property bool checked
        property bool small
        signal toggled

        implicitWidth: small ? 36 : 44
        implicitHeight: small ? 20 : 24
        radius: height / 2
        color: checked ? popup.accentColor : popup.washColor

        Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }

        Rectangle {
            width: parent.height - 6
            height: width
            radius: width / 2
            x: toggleRoot.checked ? parent.width - width - 3 : 3
            y: 3
            color: toggleRoot.checked ? popup.accentText : popup.textColor

            Behavior on x { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }
            Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }
        }

        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: toggleRoot.toggled()
        }
    }

    // Glyph, bold title and an optional subtitle, then a dim value. Children
    // go at the right end. The title column takes the slack and elides.
    component Header: RowLayout {
        id: header

        property string icon
        property string title
        property string subtitle
        property string value

        width: parent ? parent.width : implicitWidth
        spacing: 8

        SText {
            visible: header.icon !== ""
            Layout.minimumWidth: 20
            horizontalAlignment: Text.AlignHCenter
            text: header.icon
            font.pixelSize: 18
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 1

            SText {
                Layout.fillWidth: true
                text: header.title
                font.pixelSize: 14
                font.bold: true
                elide: Text.ElideRight
            }

            SText {
                Layout.fillWidth: true
                visible: header.subtitle !== ""
                text: header.subtitle
                font.pixelSize: 10
                color: popup.textDim
                elide: Text.ElideRight
            }
        }

        SText {
            visible: header.value !== ""
            text: header.value
            font.pixelSize: 10
            color: popup.textDim
        }
    }

    // A static tile: glyph, title and a dim subtitle. Height follows the text.
    component ListItem: Rectangle {
        id: tile

        property string icon
        property color iconColor: popup.textColor
        property string title
        property string subtitle

        Layout.fillWidth: true
        implicitHeight: Math.max(36, tileLines.implicitHeight + 14)
        radius: 8
        color: popup.washColor

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 10
            anchors.rightMargin: 8
            spacing: 10

            SText {
                visible: tile.icon !== ""
                Layout.minimumWidth: 18
                horizontalAlignment: Text.AlignHCenter
                text: tile.icon
                font.pixelSize: 16
                color: tile.iconColor
            }

            ColumnLayout {
                id: tileLines
                Layout.fillWidth: true
                spacing: 1

                SText {
                    Layout.fillWidth: true
                    text: tile.title
                    elide: Text.ElideRight
                }

                SText {
                    Layout.fillWidth: true
                    visible: tile.subtitle !== ""
                    text: tile.subtitle
                    font.pixelSize: 10
                    color: popup.textDim
                    elide: Text.ElideRight
                }
            }
        }
    }

    component UsageRow: AttnRow {
        textColor: popup.textColor
        textDim: popup.textDim
        washColor: popup.washColor
        borderColor: popup.borderColor
        attnColor: popup.attnColor
        attnBg: popup.attnBg
        fontFamily: popup.fontFamily
        animateBar: popup.animationsReady
    }

    // ---- surface -----------------------------------------------------------

    Rectangle {
        id: surface

        anchors.fill: parent
        implicitHeight: body.implicitHeight + 24
        color: popup.bgColor
        radius: 12
        border.color: popup.borderColor
        border.width: 1
        transformOrigin: Item.Top

        function enter() {
            opacity = 0;
            scale = 0.94;
            enterAnim.restart();
        }

        ParallelAnimation {
            id: enterAnim
            NumberAnimation { target: surface; property: "opacity"; to: 1; duration: 180; easing.type: Easing.OutCubic }
            NumberAnimation { target: surface; property: "scale"; to: 1; duration: 200; easing.type: Easing.OutCubic }
        }

        // Swallows clicks on the padding.
        MouseArea {
            anchors.fill: parent
        }

        // The content and the two sheets share one item, so the sheets cover
        // the content. Its height follows the content alone; the settings
        // sheet scrolls inside it.
        Item {
            id: body
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 12
            implicitHeight: mainCol.implicitHeight

            Column {
                id: mainCol
                width: parent.width
                spacing: 12

                Header {
                    icon: "\u{F0210}"
                    title: "attn"
                    subtitle: popup.status.date
                    value: popup.status.updatedAt ? "↻ " + popup.formatUpdatedAt(popup.status.updatedAt) : ""

                    IconButton {
                        icon: "\u{F0493}"
                        iconColor: popup.settingsOpen ? popup.attnColor : popup.textDim
                        onClicked: {
                            popup.animationsReady = false;
                            animationsReadyTimer.restart();
                            popup.settingsOpen = !popup.settingsOpen;
                        }
                    }
                }

                RowLayout {
                    width: parent.width
                    spacing: 8

                    Segmented {
                        Layout.preferredWidth: 160
                        options: [
                            { value: "today", label: "Today" },
                            { value: "week", label: "Week" }
                        ]
                        current: popup.viewMode
                        onPicked: v => {
                            popup.animationsReady = false;
                            animationsReadyTimer.restart();
                            popup.viewMode = v;
                        }
                    }

                    Item {
                        Layout.fillWidth: true
                    }

                    Rectangle {
                        id: breakChip
                        readonly property bool alert: popup.status.breakOverdue || popup.status.paused
                        readonly property int remainingSecs: Math.max(0, popup.status.breaksIntervalSecs - popup.status.activeSessionSeconds)
                        readonly property int overdueSecs: Math.max(0, popup.status.activeSessionSeconds - popup.status.breaksIntervalSecs)
                        readonly property real progress: Math.min(1.0, popup.status.activeSessionSeconds / Math.max(1, popup.status.breaksIntervalSecs))
                        readonly property string chipText: {
                            if (popup.status.paused && popup.status.pausedReason === "manual") return "paused";
                            if (popup.status.paused && popup.status.pausedReason === "idle") return "on break";
                            if (breakArea.containsMouse) return "start a break";
                            if (popup.status.breakOverdue) return overdueSecs > 0 ? "over by " + popup.formatDuration(overdueSecs) : "take a break";
                            return "break in " + popup.formatDuration(remainingSecs);
                        }

                        Layout.preferredWidth: 152
                        Layout.preferredHeight: 28
                        radius: 6
                        color: breakChip.alert ? popup.attnBg : popup.washColor
                        border.width: breakChip.alert ? 1 : 0
                        border.color: popup.attnColor
                        clip: true

                        Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }

                        SequentialAnimation on opacity {
                            running: breakChip.alert
                            loops: Animation.Infinite
                            NumberAnimation { to: 0.65; duration: 1100; easing.type: Easing.InOutSine }
                            NumberAnimation { to: 1.0; duration: 1100; easing.type: Easing.InOutSine }
                        }

                        // Subtle progress fill across the whole chip background
                        Rectangle {
                            anchors.left: parent.left
                            anchors.top: parent.top
                            anchors.bottom: parent.bottom
                            width: parent.width * breakChip.progress
                            radius: parent.radius
                            color: popup.alpha(popup.attnColor, breakChip.alert ? 0.18 : 0.10)

                            Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }
                        }

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 10
                            anchors.rightMargin: 10
                            spacing: 6

                            SText {
                                text: "\u{F0954}"
                                color: breakChip.alert ? popup.attnColor : popup.textDim
                                font.pixelSize: 11

                                Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }
                            }

                            SText {
                                Layout.fillWidth: true
                                text: breakChip.chipText
                                color: breakChip.alert ? popup.attnColor : popup.textDim
                                font.pixelSize: 11
                                font.bold: breakChip.alert
                                elide: Text.ElideRight

                                Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }
                            }
                        }

                        // While paused the break overlay covers the chip and holds the resume button.
                        MouseArea {
                            id: breakArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: popup.status.breakStart()
                        }
                    }
                }

                Column {
                    width: parent.width
                    spacing: 4
                    visible: popup.viewMode === "week"

                    Repeater {
                        model: popup.status.days

                        delegate: Rectangle {
                            id: dayItem
                            required property var modelData
                            required property int index
                            readonly property var day: modelData
                            readonly property real dayTotal: Math.max(1, Number(modelData.tracked_seconds || 0))
                            readonly property var topCats: (modelData.categories || []).slice(0, 5)

                            width: parent.width
                            implicitHeight: dayCol.implicitHeight + 8
                            radius: 8
                            color: dayHover.hovered ? popup.hoverColor : "transparent"

                            Behavior on color { ColorAnimation { duration: 100; easing.type: Easing.OutCubic } }

                            HoverHandler {
                                id: dayHover
                            }

                            ColumnLayout {
                                id: dayCol
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.leftMargin: 6
                                anchors.rightMargin: 6
                                anchors.topMargin: 4
                                spacing: 4

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 8

                                    SText {
                                        Layout.preferredWidth: 80
                                        text: popup.dayLabel(dayItem.day.date, dayItem.index)
                                        color: dayItem.index === 0 ? popup.textColor : popup.textDim
                                        font.bold: dayItem.index === 0
                                    }

                                    SText {
                                        Layout.fillWidth: true
                                        text: popup.formatDuration(Number(dayItem.day.tracked_seconds || 0))
                                    }
                                }

                                Rectangle {
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: 6
                                    radius: 3
                                    color: popup.washColor
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

                                                Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }
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
                                            id: dayCat
                                            required property var modelData
                                            spacing: 4

                                            Rectangle {
                                                anchors.verticalCenter: parent.verticalCenter
                                                width: 5
                                                height: 5
                                                radius: 2.5
                                                color: popup.categoryColor(dayCat.modelData.name)
                                            }

                                            SText {
                                                anchors.verticalCenter: parent.verticalCenter
                                                text: dayCat.modelData.name
                                                color: dayItem.index === 0 ? popup.textColor : popup.textDim
                                                font.pixelSize: 10
                                            }

                                            SText {
                                                anchors.verticalCenter: parent.verticalCenter
                                                text: popup.formatDuration(Number(dayCat.modelData.seconds || 0))
                                                color: popup.textDim
                                                font.pixelSize: 10
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
                    width: parent.width
                    implicitHeight: 36
                    radius: 8
                    color: popup.alpha(popup.attnColor, 0.10)
                    border.width: 1
                    border.color: popup.attnColor
                    visible: popup.status.breakOverdue && !popup.status.paused

                    SequentialAnimation on opacity {
                        running: overdueBanner.visible
                        loops: Animation.Infinite
                        NumberAnimation { to: 0.75; duration: 1400; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 1.0; duration: 1400; easing.type: Easing.InOutSine }
                    }

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 4
                        spacing: 10

                        SText {
                            text: "\u{F0210}"
                            color: popup.attnColor
                            font.pixelSize: 16
                        }

                        SText {
                            Layout.fillWidth: true
                            text: "Time for a break - you've been at the screen for " + popup.formatDuration(popup.status.activeSessionSeconds) + "."
                            font.pixelSize: 11
                            elide: Text.ElideRight
                        }

                        Chip {
                            text: "Take one"
                            textColor: popup.attnColor
                            onClicked: popup.status.breakStart()
                        }
                    }
                }

                Rectangle {
                    width: parent.width
                    implicitHeight: summary.implicitHeight + 24
                    radius: 8
                    color: popup.washColor
                    visible: popup.viewMode === "today"

                    ColumnLayout {
                        id: summary
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 12
                        spacing: 8

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8

                            SText {
                                text: popup.formatDuration(popup.status.trackedSeconds)
                                font.pixelSize: 20
                                font.bold: true
                            }

                            SText {
                                Layout.fillWidth: true
                                Layout.alignment: Qt.AlignBottom
                                bottomPadding: 3
                                text: "Today's focus"
                                color: popup.textDim
                                font.pixelSize: 10
                            }
                        }

                        Rectangle {
                            Layout.fillWidth: true
                            Layout.preferredHeight: 8
                            radius: 4
                            color: popup.washColor
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

                                        Behavior on color { ColorAnimation { duration: 150; easing.type: Easing.OutCubic } }

                                        // Budget threshold stripe — only visible when budget is set and not yet exceeded
                                        Rectangle {
                                            visible: barSegment.segBudget > 0 && !barSegment.modelData.over_budget && barSegment.segSeconds > 0
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
                                    id: legendItem
                                    required property var modelData
                                    spacing: 5

                                    Rectangle {
                                        anchors.verticalCenter: parent.verticalCenter
                                        width: 6
                                        height: 6
                                        radius: 3
                                        color: popup.categoryAlertColor(legendItem.modelData.name, !!legendItem.modelData.over_budget)
                                    }

                                    SText {
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: legendItem.modelData.name
                                        color: legendItem.modelData.over_budget ? popup.overBudgetColor : popup.textColor
                                        font.pixelSize: 10
                                    }

                                    SText {
                                        readonly property string budgetStr: popup.formatBudget(legendItem.modelData.seconds || 0, legendItem.modelData.budget_secs || 0)
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: budgetStr !== "" ? budgetStr : popup.formatDuration(legendItem.modelData.seconds || 0)
                                        color: legendItem.modelData.over_budget ? popup.overBudgetColor : popup.textDim
                                        font.pixelSize: 10
                                    }
                                }
                            }
                        }

                        SText {
                            Layout.fillWidth: true
                            text: "⚡ " + popup.status.switchCount + " switches · longest " + popup.formatDuration(popup.status.longestFocusSecs) + " · " + popup.status.deepWorkBlocks + " deep block" + (popup.status.deepWorkBlocks === 1 ? "" : "s")
                            color: popup.textDim
                            font.pixelSize: 10
                            elide: Text.ElideRight
                        }

                        SText {
                            Layout.fillWidth: true
                            visible: popup.status.activeSince !== ""
                            text: "🕐 active " + popup.formatUpdatedAt(popup.status.activeSince) + "–" + popup.formatUpdatedAt(popup.status.activeUntil)
                            color: popup.textDim
                            font.pixelSize: 10
                            elide: Text.ElideRight
                        }
                    }
                }

                Divider {
                    visible: popup.viewMode === "today"
                }

                RowLayout {
                    width: parent.width
                    spacing: 6
                    visible: popup.viewMode === "today"

                    Repeater {
                        model: popup.topItems()

                        delegate: ListItem {
                            required property var modelData
                            Layout.preferredWidth: 1 // equal shares, not shares by label length
                            icon: popup.iconFor(modelData.label, modelData.category, modelData.type)
                            iconColor: modelData.watched ? popup.attnColor : popup.textColor
                            title: modelData.label
                            subtitle: popup.formatDuration(modelData.seconds || 0)
                        }
                    }
                }

                Divider {
                    visible: popup.viewMode === "today"
                }

                Flow {
                    width: parent.width
                    spacing: 6
                    visible: popup.viewMode === "today" && (popup.status.categories || []).length > 0

                    Repeater {
                        model: [{name: "all"}].concat(popup.topCategories(20))

                        delegate: Chip {
                            required property var modelData
                            implicitHeight: 24
                            text: modelData.name
                            selected: (modelData.name === "all" && popup.filterCategory === "") || popup.filterCategory === modelData.name
                            textColor: selected ? popup.accentText : hovered ? popup.textColor : popup.textDim
                            onClicked: {
                                popup.animationsReady = false;
                                animationsReadyTimer.restart();
                                popup.filterCategory = (modelData.name === "all" ? "" : modelData.name);
                            }
                        }
                    }
                }

                RowLayout {
                    width: parent.width
                    spacing: 16
                    visible: popup.viewMode === "today"

                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        Layout.alignment: Qt.AlignTop
                        spacing: 6

                        Caption {
                            visible: popup.trackedApps().length > 0
                            text: "Apps"
                        }

                        ListView {
                            id: appsList
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            interactive: contentHeight > height
                            spacing: 4
                            model: popup.trackedApps()

                            delegate: UsageRow {
                                required property var modelData
                                width: Math.max(0, appsList.width - (appsList.interactive ? 8 : 0))
                                icon: popup.iconFor(modelData.id, modelData.category, "app")
                                label: popup.cleanAppName(modelData.id)
                                category: modelData.category || ""
                                seconds: modelData.seconds || 0
                                watched: !!modelData.watched
                                maxSeconds: popup.maxSeconds(popup.trackedApps())
                            }

                            ScrollBar.vertical: ScrollRail {
                                policy: appsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                            }
                        }
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: 1
                        Layout.alignment: Qt.AlignTop
                        spacing: 6

                        Caption {
                            visible: popup.trackedDomains().length > 0
                            text: "Domains"
                        }

                        ListView {
                            id: domainsList
                            Layout.fillWidth: true
                            Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            interactive: contentHeight > height
                            spacing: 4
                            model: popup.trackedDomains()

                            delegate: UsageRow {
                                required property var modelData
                                width: Math.max(0, domainsList.width - (domainsList.interactive ? 8 : 0))
                                icon: popup.iconFor(modelData.domain, modelData.category, "domain")
                                label: popup.cleanDomainName(modelData.domain)
                                category: modelData.category || ""
                                seconds: modelData.seconds || 0
                                watched: !!modelData.watched
                                maxSeconds: popup.maxSeconds(popup.trackedDomains())
                            }

                            ScrollBar.vertical: ScrollRail {
                                policy: domainsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                            }
                        }
                    }
                }

                // Other (uncategorized) drawer — shown only when there is data.
                Column {
                    width: parent.width
                    spacing: 12
                    visible: popup.viewMode === "today" && (
                        popup.uncategorizedAppsList().length > 0 ||
                        popup.uncategorizedDomainsList().length > 0
                    )

                    Divider {}

                    Column {
                        width: parent.width
                        spacing: 8

                        RowLayout {
                            width: parent.width
                            spacing: 6

                            HoverHandler {
                                cursorShape: Qt.PointingHandCursor
                            }

                            TapHandler {
                                onTapped: popup.otherExpanded = !popup.otherExpanded
                            }

                            SText {
                                text: popup.otherExpanded ? "\u{F0140}" : "\u{F0142}"
                                color: popup.textDim
                                font.pixelSize: 11
                            }

                            SText {
                                Layout.fillWidth: true
                                text: "Other"
                                font.pixelSize: 11
                                font.bold: true
                            }
                        }

                        RowLayout {
                            width: parent.width
                            spacing: 16
                            visible: popup.otherExpanded

                            // Uncategorized apps column
                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                Layout.alignment: Qt.AlignTop
                                spacing: 6
                                visible: popup.uncategorizedAppsList().length > 0

                                Caption {
                                    text: "Apps"
                                }

                                ListView {
                                    id: otherAppsList
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                                    clip: true
                                    boundsBehavior: Flickable.StopAtBounds
                                    interactive: contentHeight > height
                                    spacing: 4
                                    model: popup.uncategorizedAppsList()

                                    delegate: RowLayout {
                                        id: otherApp
                                        required property var modelData
                                        readonly property bool isSynthetic: modelData.id === "(below threshold)"
                                        width: Math.max(0, otherAppsList.width - (otherAppsList.interactive ? 8 : 0))
                                        spacing: 4

                                        UsageRow {
                                            Layout.fillWidth: true
                                            icon: "\u{F01C4}"
                                            label: popup.cleanAppName(otherApp.modelData.id)
                                            category: ""
                                            seconds: otherApp.modelData.seconds || 0
                                            watched: false
                                            maxSeconds: popup.maxSeconds(popup.uncategorizedAppsList())
                                        }

                                        IconButton {
                                            visible: !otherApp.isSynthetic
                                            size: 22
                                            icon: "+"
                                            iconColor: popup.textDim
                                            onClicked: {
                                                popup.categoryPickerKind = "app";
                                                popup.categoryPickerItemId = otherApp.modelData.id;
                                                categoryPickerMenu.popup();
                                            }
                                        }
                                    }

                                    ScrollBar.vertical: ScrollRail {
                                        policy: otherAppsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                                    }
                                }
                            }

                            // Uncategorized domains column
                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.preferredWidth: 1
                                Layout.alignment: Qt.AlignTop
                                spacing: 6
                                visible: popup.uncategorizedDomainsList().length > 0

                                Caption {
                                    text: "Domains"
                                }

                                ListView {
                                    id: otherDomainsList
                                    Layout.fillWidth: true
                                    Layout.preferredHeight: Math.min(contentHeight, popup.todayListMaxHeight)
                                    clip: true
                                    boundsBehavior: Flickable.StopAtBounds
                                    interactive: contentHeight > height
                                    spacing: 4
                                    model: popup.uncategorizedDomainsList()

                                    delegate: RowLayout {
                                        id: otherDomain
                                        required property var modelData
                                        readonly property bool isSynthetic: modelData.domain === "(below threshold)"
                                        width: Math.max(0, otherDomainsList.width - (otherDomainsList.interactive ? 8 : 0))
                                        spacing: 4

                                        UsageRow {
                                            Layout.fillWidth: true
                                            icon: "\u{F059F}"
                                            label: popup.cleanDomainName(otherDomain.modelData.domain)
                                            category: ""
                                            seconds: otherDomain.modelData.seconds || 0
                                            watched: false
                                            maxSeconds: popup.maxSeconds(popup.uncategorizedDomainsList())
                                        }

                                        IconButton {
                                            visible: !otherDomain.isSynthetic
                                            size: 22
                                            icon: "+"
                                            iconColor: popup.textDim
                                            onClicked: {
                                                popup.categoryPickerKind = "domain";
                                                popup.categoryPickerItemId = otherDomain.modelData.domain;
                                                categoryPickerMenu.popup();
                                            }
                                        }
                                    }

                                    ScrollBar.vertical: ScrollRail {
                                        policy: otherDomainsList.interactive ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Item {
                id: settingsOverlay
                anchors.fill: parent
                visible: popup.settingsOpen
                opacity: visible ? 1.0 : 0.0
                Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

                // The backdrop reaches over the padding to the hairline border,
                // so the sheet hides the whole popup, not just the content.
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 1 - 12
                    radius: 11
                    color: popup.alpha(popup.bgColor, 0.96)

                    MouseArea {
                        anchors.fill: parent
                    }
                }

                // Header pinned at top
                Header {
                    id: settingsHeader
                    anchors.top: parent.top
                    icon: "\u{F0493}"
                    title: "Settings"
                    opacity: settingsOverlay.visible ? 1.0 : 0.0
                    scale: settingsOverlay.visible ? 1.0 : 0.94

                    Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }

                    IconButton {
                        icon: "×"
                        iconColor: popup.textDim
                        onClicked: {
                            popup.settingsOpen = false;
                            popup.status.restartHint = "";
                        }
                    }
                }

                // Scrollable settings content. It reaches 8px into the right
                // padding, so the scroll rail sits there instead of over controls.
                Flickable {
                    id: settingsFlickable
                    anchors.top: settingsHeader.bottom
                    anchors.topMargin: 12
                    anchors.bottom: parent.bottom
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.rightMargin: -8
                    clip: true
                    contentWidth: width
                    contentHeight: settingsContent.implicitHeight
                    boundsBehavior: Flickable.StopAtBounds

                    ScrollBar.vertical: ScrollRail {
                        policy: settingsFlickable.contentHeight > settingsFlickable.height
                                ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
                    }

                    Column {
                        id: settingsContent
                        width: settingsFlickable.width - 8
                        spacing: 12
                        opacity: settingsOverlay.visible ? 1.0 : 0.0
                        scale: settingsOverlay.visible ? 1.0 : 0.94

                        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                        Behavior on scale { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }

                        // ── Break reminder ────────────────────────────────────────
                        Column {
                            width: parent.width
                            spacing: 8

                            Header {
                                title: "Break reminder"

                                Toggle {
                                    checked: popup.status.breaksEnabled
                                    onToggled: popup.status.setBreaks(!popup.status.breaksEnabled, popup.status.breaksIntervalSecs, popup.status.breaksMinBreakSecs)
                                }
                            }

                            Column {
                                width: parent.width
                                spacing: 6
                                opacity: popup.status.breaksEnabled ? 1.0 : 0.5

                                Behavior on opacity { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }

                                Caption {
                                    text: "Prompt after"
                                }

                                Segmented {
                                    width: parent.width
                                    options: [
                                        { value: 1800, label: "30m" },
                                        { value: 3600, label: "1h" },
                                        { value: 5400, label: "90m" },
                                        { value: 7200, label: "2h" },
                                        { value: 10800, label: "3h" }
                                    ]
                                    current: popup.status.breaksIntervalSecs
                                    onPicked: v => popup.status.setBreaks(popup.status.breaksEnabled, v, popup.status.breaksMinBreakSecs)
                                }
                            }

                            Column {
                                width: parent.width
                                spacing: 6
                                opacity: popup.status.breaksEnabled ? 1.0 : 0.5

                                Behavior on opacity { NumberAnimation { duration: 150; easing.type: Easing.OutCubic } }

                                Caption {
                                    text: "Idle is a break after"
                                }

                                Segmented {
                                    width: parent.width
                                    options: [
                                        { value: 120, label: "2m" },
                                        { value: 300, label: "5m" },
                                        { value: 600, label: "10m" },
                                        { value: 900, label: "15m" }
                                    ]
                                    current: popup.status.breaksMinBreakSecs
                                    onPicked: v => popup.status.setBreaks(popup.status.breaksEnabled, popup.status.breaksIntervalSecs, v)
                                }
                            }
                        }

                        Divider {}

                        // ── Budgets ───────────────────────────────────────────────
                        Column {
                            width: parent.width
                            spacing: 8
                            visible: (popup.status.categories || []).length > 0

                            Header {
                                title: "Budgets"
                                subtitle: "Click a preset to set or clear a daily budget"
                            }

                            Repeater {
                                model: popup.status.categories || []

                                delegate: ColumnLayout {
                                    id: catRow
                                    required property var modelData
                                    readonly property int catBudget: (modelData.budget_secs && modelData.budget_secs > 0) ? modelData.budget_secs : 0
                                    width: parent.width
                                    spacing: 4

                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 8

                                        Rectangle {
                                            Layout.preferredWidth: 7
                                            Layout.preferredHeight: 7
                                            Layout.alignment: Qt.AlignVCenter
                                            radius: 3.5
                                            color: popup.categoryColor(catRow.modelData.name)
                                        }

                                        SText {
                                            Layout.fillWidth: true
                                            text: catRow.modelData.name
                                            elide: Text.ElideRight
                                        }

                                        SText {
                                            text: catRow.catBudget > 0
                                                  ? popup.formatDuration(catRow.catBudget) + "/day"
                                                  : "no limit"
                                            color: catRow.catBudget > 0 ? popup.attnColor : popup.textDim
                                            font.pixelSize: 10
                                        }
                                    }

                                    Segmented {
                                        Layout.fillWidth: true
                                        Layout.leftMargin: 15
                                        chipHeight: 24
                                        options: [
                                            { value: 0, label: "off" },
                                            { value: 900, label: "15m" },
                                            { value: 1800, label: "30m" },
                                            { value: 3600, label: "1h" },
                                            { value: 7200, label: "2h" },
                                            { value: 14400, label: "4h" },
                                            { value: 28800, label: "8h" }
                                        ]
                                        current: catRow.catBudget
                                        onPicked: v => popup.status.setBudget(catRow.modelData.name, v)
                                    }
                                }
                            }
                        }

                        Divider {
                            visible: (popup.status.categories || []).length > 0
                        }

                        // ── Notifications ─────────────────────────────────────────
                        Column {
                            width: parent.width
                            spacing: 8

                            Header {
                                title: "Notifications"
                            }

                            Repeater {
                                model: [
                                    { label: "Enable notifications", field: "enabled" },
                                    { label: "Break-overdue alerts", field: "breakOverdue" },
                                    { label: "Budget-exceeded alerts", field: "budgetExceeded" }
                                ]

                                delegate: RowLayout {
                                    id: notifRow
                                    required property var modelData
                                    readonly property bool fieldValue: {
                                        switch (modelData.field) {
                                            case "enabled":       return popup.status.notificationsEnabled;
                                            case "breakOverdue":  return popup.status.notificationsBreakOverdue;
                                            case "budgetExceeded":return popup.status.notificationsBudgetExceeded;
                                            default: return false;
                                        }
                                    }

                                    width: parent.width
                                    spacing: 8

                                    Caption {
                                        Layout.fillWidth: true
                                        text: notifRow.modelData.label
                                    }

                                    Toggle {
                                        small: true
                                        checked: notifRow.fieldValue
                                        onToggled: {
                                            var e = popup.status.notificationsEnabled;
                                            var b = popup.status.notificationsBreakOverdue;
                                            var u = popup.status.notificationsBudgetExceeded;
                                            switch (notifRow.modelData.field) {
                                                case "enabled":        e = !e; break;
                                                case "breakOverdue":   b = !b; break;
                                                case "budgetExceeded": u = !u; break;
                                            }
                                            popup.status.setNotifications(e, b, u);
                                        }
                                    }
                                }
                            }
                        }

                        Divider {}

                        // ── Compositor / Focus source ─────────────────────────────
                        Column {
                            width: parent.width
                            spacing: 8

                            Header {
                                title: "Compositor"
                                subtitle: "Focus source (requires daemon restart)"
                            }

                            Segmented {
                                width: parent.width
                                options: ["auto", "niri", "hyprland", "river", "sway"]
                                current: popup.status.focusSourceKind
                                onPicked: v => popup.status.setFocusSource(v)
                            }

                            SText {
                                width: parent.width
                                visible: popup.status.restartHint !== ""
                                text: popup.status.restartHint
                                color: popup.attnColor
                                font.pixelSize: 10
                                wrapMode: Text.WordWrap
                            }
                        }

                        // Footer hint
                        SText {
                            width: parent.width
                            horizontalAlignment: Text.AlignHCenter
                            text: "Edit ~/.config/attn/config.toml for watch lists.\nDaemon auto-reloads on save."
                            color: popup.textDim
                            font.pixelSize: 10
                            wrapMode: Text.WordWrap
                        }
                    }
                }
            }

            Item {
                id: breakOverlay
                anchors.fill: parent
                visible: popup.status.paused
                opacity: visible ? 1.0 : 0.0
                Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 1 - 12
                    radius: 11
                    color: popup.alpha(popup.bgColor, 0.94)

                    MouseArea { anchors.fill: parent } // swallow clicks behind
                }

                ColumnLayout {
                    id: overlayContent
                    anchors.centerIn: parent
                    width: parent.width * 0.7
                    spacing: 12
                    opacity: breakOverlay.visible ? 1.0 : 0.0
                    scale: breakOverlay.visible ? 1.0 : 0.92

                    Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: 200; easing.type: Easing.OutBack } }

                    SText {
                        Layout.alignment: Qt.AlignHCenter
                        text: "\u{F0210}"
                        color: popup.attnColor
                        font.pixelSize: 36
                    }

                    SText {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        font.pixelSize: 14
                        font.bold: true
                        text: popup.status.pausedReason === "idle" ? "You're on a break" : "Tracking paused"
                    }

                    SText {
                        Layout.fillWidth: true
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        color: popup.textDim
                        text: popup.status.pausedReason === "idle"
                            ? "Auto-paused after " + popup.formatDuration(popup.status.breaksMinBreakSecs) + " without input. Move the mouse to resume."
                            : "Resumes on its own when you're back from " + popup.formatDuration(popup.status.breaksMinBreakSecs) + " away, or press resume."
                    }

                    Chip {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 168
                        Layout.preferredHeight: 36
                        visible: popup.status.pausedReason !== "idle"
                        text: "Resume tracking"
                        textColor: popup.attnColor
                        onClicked: popup.status.breakEnd()
                    }
                }
            }
        }

        // A Popup needs an Item parent to open from, so the Menu sits in the
        // surface rather than on the window.
        Menu {
            id: categoryPickerMenu
            title: "Add to category"

            Instantiator {
                model: popup.status.categories
                delegate: MenuItem {
                    required property var modelData
                    text: modelData.name || ""
                    onTriggered: popup.status.categorize(popup.categoryPickerKind, popup.categoryPickerItemId, modelData.name)
                }
                onObjectAdded: (index, object) => categoryPickerMenu.insertItem(index, object)
                onObjectRemoved: (index, object) => categoryPickerMenu.removeItem(object)
            }
        }
    }
}
