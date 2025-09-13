import sys
import numpy as np
import sounddevice as sd
from PyQt6 import QtWidgets, QtGui, QtCore
from PyQt6.QtWidgets import QGraphicsDropShadowEffect
from PyQt6.QtGui import QColor


# ==== Waveform Widget ====
class WaveformWidget(QtWidgets.QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.data = np.zeros(1024)  # empty buffer
        self.setMinimumHeight(100)
        self.setStyleSheet("background-color: #1e1b22; border-radius: 10px;")

    def update_waveform(self, new_data):
        self.data = new_data
        self.update()  # trigger repaint

    def paintEvent(self, event):
        if self.data is None or len(self.data) == 0:
            return
        painter = QtGui.QPainter(self)
        painter.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)

        pen = QtGui.QPen(QtGui.QColor("#ae1d6f"))
        pen.setWidth(1)
        painter.setPen(pen)

        w = self.width()
        h = self.height()
        mid = h // 2

        # downsample for performance
        step = max(1, len(self.data) // w)
        points = []
        for i in range(0, len(self.data), step):
            x = int(i / len(self.data) * w)
            y = int(mid - self.data[i] * (h // 3))  # scale
            points.append(QtCore.QPointF(x, y))

        if points:
            painter.drawPolyline(*points)


# ==== Main Mic UI ====
class MicUI(QtWidgets.QWidget):
    def __init__(self):
        super().__init__()
        # 🔹 Initialize active state
        self.active = False   
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

        # === Small logo + text ===
        row_layout = QtWidgets.QHBoxLayout()
        row_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)

        small_logo = QtWidgets.QLabel()
        pixmap_small = QtGui.QPixmap("icon/app_logo.png")
        pixmap_small = pixmap_small.scaled(50, 50, QtCore.Qt.AspectRatioMode.KeepAspectRatio,
                                           QtCore.Qt.TransformationMode.SmoothTransformation)
        small_logo.setPixmap(pixmap_small)

        small_text = QtWidgets.QLabel("JarviSonix")
        small_text.setStyleSheet("font-size: 18px; font-weight: bold; color: #ae1d6f;")

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

        glow_effect = QGraphicsDropShadowEffect()
        glow_effect.setBlurRadius(40)
        glow_effect.setOffset(0)
        glow_effect.setColor(QColor(174, 29, 111, 200))
        section_frame.setGraphicsEffect(glow_effect)

        # Use QVBoxLayout with spacers for fixed positioning
        frame_layout = QtWidgets.QVBoxLayout()
        frame_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignTop)
        section_frame.setLayout(frame_layout)

        # ---- Status Label ----
        self.status_label = QtWidgets.QLabel("OFF")
        self.status_label.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
        self.status_label.setStyleSheet("""
            margin-top: 40px;
            font-size: 16px;
            font-weight: bold;
            color: #ffffff;
            background: transparent;
            border: none;
        """)
        frame_layout.addWidget(self.status_label)

        frame_layout.addSpacing(20)  # space between label and mic button

        # ---- Microphone button ----
        self.mic_button = QtWidgets.QPushButton()
        self.mic_button.setFixedSize(100, 100)
        self.mic_button.setIcon(QtGui.QIcon("icon/mic.svg"))
        self.mic_button.setIconSize(QtCore.QSize(50, 50))
        self.mic_button.clicked.connect(self.toggle_active)
        frame_layout.addWidget(self.mic_button, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)

        frame_layout.addSpacing(20)  # space between mic button and play/pause


        # ---- Play/Pause button ----
        self.play_pause_button = QtWidgets.QPushButton()
        self.play_pause_button.setFixedSize(50, 50)
        self.play_pause_button.setIcon(QtGui.QIcon("icon/play.svg"))
        self.play_pause_button.setIconSize(QtCore.QSize(30, 30))
        self.play_pause_button.clicked.connect(self.toggle_active)
        self.play_pause_button.setStyleSheet("""
            QPushButton {
                background-color: #3c2d40;
            }
            QPushButton:hover {
                background-color: #5e3b5e;
            }
        """)
        frame_layout.addWidget(self.play_pause_button, alignment=QtCore.Qt.AlignmentFlag.AlignCenter)

        frame_layout.addSpacing(20)  # space between buttons and waveform

        # ---- Waveform (reserve space) ----
        self.waveform = WaveformWidget()
        self.waveform.setFixedHeight(120)   # reserve space, won't change layout
        self.waveform.setVisible(False)     # start hidden
        frame_layout.addWidget(self.waveform)

        # ---- Add spacer at the bottom to prevent other layout shifts ----
        frame_layout.addStretch()

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

        small_text = QtWidgets.QLabel("HTN 2025 Hacker Project :3")
        small_text.setStyleSheet("font-size: 12px; font-weight: bold; color: #ae1d6f;")

        row_layout.addWidget(small_logo)
        row_layout.addSpacing(8)
        row_layout.addWidget(small_text)

        main_layout.addLayout(row_layout)

        # ---- Setup audio input ----
        self.stream = sd.InputStream(callback=self.audio_callback, channels=1, samplerate=44100)
        self.stream.start()

    # ---- Audio callback updates waveform ----
    def audio_callback(self, indata, frames, time, status):
        if status:
            print(status)
        samples = indata[:, 0][::4]  # downsample
        self.waveform.update_waveform(samples)

    # ---- Toggle shared state ----
    def toggle_active(self):
        self.active = not self.active
        self.update_mic_style()
        self.update_play_pause()
        self.update_status_label()

        # 🔹 Show waveform only when active
        self.waveform.setVisible(self.active)

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
