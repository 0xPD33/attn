import QtQuick
import QtQuick.Layouts

Rectangle {
    id: row

    property string label: ""
    property string icon: ""
    property string category: ""
    property int seconds: 0
    property bool watched: false
    property color textColor: "#d4d4dc"
    property color textDim: "#868690"
    property color accentColor: "#7c7c88"
    property color bgSecondary: "#24242c"
    property color watchedAccent: "#c9b563"
    property color watchedBg: "#2d2820"
    property string fontFamily: "JetBrainsMono Nerd Font"
    property bool animateBar: true

    property int maxSeconds: 0

    implicitHeight: 24
    radius: 6
    color: hoverArea.containsMouse ? Qt.rgba(row.accentColor.r, row.accentColor.g, row.accentColor.b, 0.08) : "transparent"

    Behavior on color { ColorAnimation { duration: 100; easing.type: Easing.OutCubic } }

    function formatDuration(s) {
        if (s < 60) return String(s) + "s";
        var m = Math.floor(s / 60);
        if (m < 60) return String(m) + "m";
        var h = Math.floor(m / 60);
        var rest = m % 60;
        return rest > 0 ? String(h) + "h " + String(rest) + "m" : String(h) + "h";
    }

    MouseArea {
        id: hoverArea
        anchors.fill: parent
        hoverEnabled: true
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 6
        anchors.rightMargin: 6
        anchors.topMargin: 3
        anchors.bottomMargin: 3
        spacing: 6

        Rectangle {
            Layout.preferredWidth: 18
            Layout.preferredHeight: 18
            Layout.alignment: Qt.AlignVCenter
            radius: 9
            color: row.watched ? row.watchedBg : row.bgSecondary
            border.width: 1
            border.color: row.watched ? row.watchedAccent : row.accentColor

            Text {
                id: rowGlyph
                anchors.centerIn: parent
                text: row.icon
                color: row.watched ? row.watchedAccent : row.textDim
                font.family: row.fontFamily
                font.pixelSize: 11
            }
        }

        Text {
            text: row.label
            color: row.watched ? row.textColor : row.textDim
            font.family: row.fontFamily
            font.pixelSize: 11
            font.weight: row.watched ? Font.DemiBold : Font.Normal
            elide: Text.ElideRight
            Layout.fillWidth: true
        }

        Rectangle {
            Layout.preferredWidth: 48
            Layout.preferredHeight: 4
            radius: 2
            color: Qt.darker(row.bgSecondary, 1.15)

            Rectangle {
                width: Math.max(2, parent.width * row.seconds / Math.max(1, row.maxSeconds))
                height: parent.height
                radius: 2
                color: row.watched ? row.watchedAccent : row.accentColor
                opacity: row.watched ? 1.0 : 0.55

                Behavior on color { ColorAnimation { duration: 200; easing.type: Easing.OutCubic } }
            }
        }

        Text {
            text: row.formatDuration(row.seconds)
            color: row.watched ? row.textColor : row.textDim
            font.family: row.fontFamily
            font.pixelSize: 10
            horizontalAlignment: Text.AlignRight
            Layout.preferredWidth: 44
        }
    }
}
