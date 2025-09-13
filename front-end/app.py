import sys
from PyQt6 import QtWidgets, QtGui, QtCore

class MicUI(QtWidgets.QWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Microphone UI")
        self.resize(300, 400)
        self.setStyleSheet("background-color: #2b2b2b;")  # Dark background

        # Main vertical layout
        main_layout = QtWidgets.QVBoxLayout()
        main_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        self.setLayout(main_layout)

        # Add vertical stretch to center the frame vertically
        main_layout.addStretch(1)

        # ---- Section Frame ----
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

        # Layout inside the frame
        frame_layout = QtWidgets.QVBoxLayout()
        frame_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        section_frame.setLayout(frame_layout)

        # ---- Status Label ----
        self.status_label = QtWidgets.QLabel("Mic OFF / Paused")
        self.status_label.setStyleSheet("color: #ffffff; font-size: 16px; font-weight: bold;")
        self.status_label.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        frame_layout.addWidget(self.status_label)

        frame_layout.addSpacing(10)

        # ---- Microphone button (toggle) ----
        self.active = False
        self.mic_button = QtWidgets.QPushButton()
        self.mic_button.setFixedSize(100, 100)
        self.mic_button.setIcon(QtGui.QIcon("icon/mic.svg"))
        self.mic_button.setIconSize(QtCore.QSize(50, 50))
        self.update_mic_style()
        self.mic_button.clicked.connect(self.toggle_active)

        # ✅ Center via layout, not setAlignment
        frame_layout.addWidget(self.mic_button, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)

        frame_layout.addSpacing(20)

        # ---- Play/Pause button ----
        self.play_pause_button = QtWidgets.QPushButton()
        self.play_pause_button.setIcon(QtGui.QIcon("icon/play.svg"))
        self.play_pause_button.setFixedSize(50, 50)
        self.play_pause_button.clicked.connect(self.toggle_active)
        self.play_pause_button.setIconSize(QtCore.QSize(30, 30))

        # ✅ Center via layout
        frame_layout.addWidget(self.play_pause_button, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)

        main_layout.addWidget(section_frame)

        # Add vertical stretch after the frame to center vertically
        main_layout.addStretch(1)

    # ---- Toggle shared state ----
    def toggle_active(self):
        self.active = not self.active
        self.update_mic_style()
        self.update_play_pause()
        self.update_status_label()
        print("ON" if self.active else "OFF")

    def update_mic_style(self):
        if self.active:
            self.mic_button.setStyleSheet("""
                QPushButton {
                    border-radius: 50px;
                    background-color: #4caf50;
                }
                QPushButton:hover {
                    background-color: #66bb6a;
                }
            """)
        else:
            self.mic_button.setStyleSheet("""
                QPushButton {
                    border-radius: 50px;
                    background-color: #888888;
                }
                QPushButton:hover {
                    background-color: #aaaaaa;
                }
            """)

    def update_play_pause(self):
        icon = "icon/pause.svg" if self.active else "icon/play.svg"
        self.play_pause_button.setIcon(QtGui.QIcon(icon))

    def update_status_label(self):
        self.status_label.setText("Mic ON / Playing" if self.active else "Mic OFF / Paused")


if __name__ == "__main__":
    app = QtWidgets.QApplication(sys.argv)
    window = MicUI()
    window.show()
    sys.exit(app.exec())
