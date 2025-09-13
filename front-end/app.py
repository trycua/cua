import sys
from PyQt6 import QtWidgets, QtGui, QtCore
from PyQt6.QtWidgets import QGraphicsDropShadowEffect
from PyQt6.QtGui import QColor

class MicUI(QtWidgets.QWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Microphone UI")
        self.resize(320, 550)

        # Global dark magenta material style
        self.setStyleSheet("""
            QWidget {
                background-color: #1e1b22;
                font-family: "Courier New", monospace;
                color: #ffffff;
            }
            QLabel {
                font-family: "Courier New", monospace;
            }
            QPushButton {
                font-family: "Courier New", monospace;
                border: none;
                color: white;
            }
            QPushButton:pressed {
                background-color: #ae1d6f;
            }
        """)

        # Main vertical layout
        main_layout = QtWidgets.QVBoxLayout()
        main_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        self.setLayout(main_layout)

        # === Small logo + text side by side ===
        row_layout = QtWidgets.QHBoxLayout()
        row_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        small_logo = QtWidgets.QLabel()
        pixmap_small = QtGui.QPixmap("icon/app_logo.png")
        pixmap_small = pixmap_small.scaled(50, 50, QtCore.Qt.AspectRatioMode.KeepAspectRatio,
                                           QtCore.Qt.TransformationMode.SmoothTransformation)
        small_logo.setPixmap(pixmap_small)
        small_logo.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        small_text = QtWidgets.QLabel("JarviSonix")
        small_text.setStyleSheet("font-size: 18px; font-weight: bold; color: #ae1d6f;")
        small_text.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        row_layout.addWidget(small_logo)
        row_layout.addSpacing(8)
        row_layout.addWidget(small_text)

        main_layout.addLayout(row_layout)

        # ---- Section Frame ----
        section_frame = QtWidgets.QFrame()
        section_frame.setFixedSize(320, 520)
        section_frame.setStyleSheet("""
            QFrame {
                border: 2px solid #ae1d6f;
                border-radius: 20px;
                background-color: #2b2230;
            }
        """)

        # Glow effect for the frame
        glow_effect = QGraphicsDropShadowEffect()
        glow_effect.setBlurRadius(40)
        glow_effect.setOffset(0)
        glow_effect.setColor(QColor(174, 29, 111, 200))  # dark magenta glow
        section_frame.setGraphicsEffect(glow_effect)

        # Layout inside the frame
        frame_layout = QtWidgets.QVBoxLayout()
        frame_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        section_frame.setLayout(frame_layout)

        # ---- Status Label ----
        self.status_label = QtWidgets.QLabel("OFF")
        self.status_label.setStyleSheet("font-size: 16px; font-weight: bold; color: #ffffff; font-border: none;")
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

        frame_layout.addWidget(self.mic_button, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)
        frame_layout.addSpacing(20)

        # ---- Play/Pause button ----
        self.play_pause_button = QtWidgets.QPushButton()
        self.play_pause_button.setIcon(QtGui.QIcon("icon/play.svg"))
        self.play_pause_button.setFixedSize(50, 50)
        self.play_pause_button.clicked.connect(self.toggle_active)
        self.play_pause_button.setIconSize(QtCore.QSize(30, 30))

        # Add pink/purple border style
        self.play_pause_button.setStyleSheet("""
            QPushButton {
                background-color: #3c2d40;
            }
            QPushButton:hover {
                background-color: #5e3b5e;
            }
        """)

        frame_layout.addWidget(self.play_pause_button, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)


        main_layout.addWidget(section_frame)
        frame_layout.addSpacing(50)

        # === Footer logo + text ===
        row_layout = QtWidgets.QHBoxLayout()
        row_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        small_logo = QtWidgets.QLabel()
        pixmap_small = QtGui.QPixmap("icon/hackthenorth.png")
        pixmap_small = pixmap_small.scaled(30, 30, QtCore.Qt.AspectRatioMode.KeepAspectRatio,
                                           QtCore.Qt.TransformationMode.SmoothTransformation)
        small_logo.setPixmap(pixmap_small)
        small_logo.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        small_text = QtWidgets.QLabel("HTN 2025 Hacker Project :3")
        small_text.setStyleSheet("font-size: 12px; font-weight: bold; color: #ae1d6f;")
        small_text.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        row_layout.addWidget(small_logo)
        row_layout.addSpacing(8)
        row_layout.addWidget(small_text)

        main_layout.addLayout(row_layout)

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
                    background: qlineargradient(
                        x1:0, y1:0, x2:1, y2:1,
                        stop:0.33 #d91da5,
                        stop:0.76 #a322c7
                    );
                }
                QPushButton:hover {
                    background: qlineargradient(
                        x1:0, y1:0, x2:1, y2:1,
                        stop:0.33 #d91da5,
                        stop:0.76 #a322c7
                    );
                    opacity: 0.8;
                }
            """)
            glow = QGraphicsDropShadowEffect(self.mic_button)
            glow.setBlurRadius(25)
            glow.setColor(QColor("#ae1d6f"))
            glow.setOffset(0)
            self.mic_button.setGraphicsEffect(glow)
        else:
            self.mic_button.setStyleSheet("""
                QPushButton {
                    border-radius: 50px;
                    background-color: #555555;
                }
                QPushButton:hover {
                    background-color: #777777;
                }
            """)
            self.mic_button.setGraphicsEffect(None)

    def update_play_pause(self):
        icon = "icon/pause.svg" if self.active else "icon/play.svg"
        self.play_pause_button.setIcon(QtGui.QIcon(icon))

    def update_status_label(self):
        self.status_label.setText("ON" if self.active else "OFF")


if __name__ == "__main__":
    app = QtWidgets.QApplication(sys.argv)
    window = MicUI()
    window.show()
    sys.exit(app.exec())
