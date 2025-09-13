from PyQt6 import QtWidgets, QtGui, QtCore


class MicUI(QtWidgets.QWidget):
    def __init__(self):
        super().__init__()
        self.setStyleSheet("background-color: #2b2b2b;")  # Dark background

        main_layout = QtWidgets.QVBoxLayout()
        main_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        self.setLayout(main_layout)

        section_frame = QtWidgets.QFrame()
        section_frame.setFixedSize(250, 250)
        section_frame.setStyleSheet("""
            QFrame {
                border: 2px solid #555;
                border-radius: 15px;
                background-color: #3c3c3c;
            }
        """)

        shadow = QtWidgets.QGraphicsDropShadowEffect()
        shadow.setBlurRadius(15)
        shadow.setOffset(3, 3)
        shadow.setColor(QtGui.QColor(0, 0, 0, 160))
        section_frame.setGraphicsEffect(shadow)

        frame_layout = QtWidgets.QVBoxLayout()
        frame_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        section_frame.setLayout(frame_layout)

        self.mic_button = QtWidgets.QPushButton()
        self.mic_button.setFixedSize(100, 100)
        self.mic_button.setIcon(QtGui.QIcon("icon/mic.svg"))  # Replace with your mic icon
        self.mic_button.setIconSize(QtCore.QSize(50, 50))
        self.mic_button.setStyleSheet("""
            QPushButton {
                border-radius: 50px;
                background-color: #4caf50;
            }
            QPushButton:hover {
                background-color: #66bb6a;
            }
        """)
        frame_layout.addWidget(self.mic_button)

        frame_layout.addSpacing(20)

        controls_layout = QtWidgets.QHBoxLayout()

        self.playing = False
        self.play_pause_button = QtWidgets.QPushButton()
        self.play_pause_button.setIcon(QtGui.QIcon("icon/play.svg"))  # initial icon
        self.play_pause_button.setFixedSize(50, 50)
        self.play_pause_button.setIconSize(QtCore.QSize(30, 30))
        self.play_pause_button.clicked.connect(self.toggle_play_pause)
        controls_layout.addWidget(self.play_pause_button)

        self.stop_button = QtWidgets.QPushButton()
        self.stop_button.setIcon(QtGui.QIcon("icon/stop.svg"))
        self.stop_button.setFixedSize(50, 50)
        self.stop_button.setIconSize(QtCore.QSize(30, 30))
        controls_layout.addWidget(self.stop_button)

        frame_layout.addLayout(controls_layout)
        main_layout.addWidget(section_frame)

    def toggle_play_pause(self):
        self.playing = not self.playing
        icon = "icon/pause.svg" if self.playing else "icon/play.svg"
        self.play_pause_button.setIcon(QtGui.QIcon(icon))
        print("Playing" if self.playing else "Paused")
