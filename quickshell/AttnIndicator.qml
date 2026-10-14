import QtQuick

Item {
    id: root

    // Owns one status object by default. Pass the same object to AttnPopup.
    property AttnStatus status: AttnStatus {}

    property color textColor: "#d4d4dc"
    property color washColor: Qt.rgba(1, 1, 1, 0.07)
    property color attnColor: "#c9b563"
    property color dotColor: "#d8b45c"
    property string fontFamily: "JetBrainsMono Nerd Font"

    signal clicked()

    implicitWidth: chip.implicitWidth
    implicitHeight: 30
    width: implicitWidth
    height: implicitHeight

    Rectangle {
        id: chip
        anchors.fill: parent
        implicitWidth: 30
        implicitHeight: 30
        radius: 8
        readonly property bool alert: root.status.breakOverdue || root.status.paused
        // No container at rest: the alert state lives in the glyph colour.
        color: chipArea.containsMouse ? Qt.rgba(root.washColor.r, root.washColor.g, root.washColor.b, Math.min(1, root.washColor.a + 0.03)) : "transparent"
        opacity: root.status.stale ? 0.45 : 1.0

        Behavior on color { ColorAnimation { duration: 120; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }

        SequentialAnimation on opacity {
            running: chip.alert && !root.status.stale
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
                color: chip.alert ? root.attnColor : root.textColor
                font.family: root.fontFamily
                font.pixelSize: 16
                font.weight: Font.DemiBold
            }

            Rectangle {
                anchors.right: parent.right
                anchors.top: parent.top
                width: 6
                height: 6
                radius: 3
                color: root.dotColor
                visible: root.status.trackedSeconds > 0
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
