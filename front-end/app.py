import sys
from PyQt6 import QtWidgets
from music_ui import MusicUI
from mic_ui import MicUI


class MainWindow(QtWidgets.QWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Music + Microphone UI")
        self.setGeometry(100, 100, 1200, 600)
        self.setStyleSheet("background-color: #2b2b2b;")

        layout = QtWidgets.QHBoxLayout()
        self.setLayout(layout)

        # Right: MicUI
        layout.addWidget(MicUI())


if __name__ == "__main__":
    app = QtWidgets.QApplication(sys.argv)
    window = MainWindow()
    window.show()
    sys.exit(app.exec())
