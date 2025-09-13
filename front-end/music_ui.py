from PyQt6 import QtWidgets, QtGui, QtCore


class MusicUI(QtWidgets.QWidget):
    def __init__(self):
        super().__init__()
        self.setStyleSheet("background-color: #2b2b2b;")  # Dark background

        main_layout = QtWidgets.QHBoxLayout()
        self.setLayout(main_layout)

        # --------------------------
        # Column 1: Icons (Instruments)
        # --------------------------
        icons_layout = QtWidgets.QVBoxLayout()
        icons_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignTop)

        instruments = [
            ("Piano", "icon/piano.svg"),
            ("Guitar", "icon/guitar.svg"),
            ("Synth", "icon/synth.svg")
        ]

        for name, icon_path in instruments:
            btn = QtWidgets.QPushButton()
            btn.setIcon(QtGui.QIcon(icon_path))  # Replace with your icons
            btn.setIconSize(QtCore.QSize(40, 40))
            btn.setFixedSize(60, 60)
            btn.setToolTip(name)
            btn.setStyleSheet("""
                QPushButton {
                    border-radius: 10px;
                    background-color: #444;
                }
                QPushButton:hover {
                    background-color: #555;
                }
            """)
            icons_layout.addWidget(btn)

        main_layout.addLayout(icons_layout)

        # --------------------------
        # Column 2: Track Area
        # --------------------------
        track_layout = QtWidgets.QVBoxLayout()
        track_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignTop)

        for name, _ in instruments:
            track_frame = QtWidgets.QFrame()
            track_frame.setFixedHeight(60)
            track_frame.setStyleSheet("""
                QFrame {
                    border: 1px solid #555;
                    border-radius: 5px;
                    background-color: #3c3c3c;
                }
            """)

            clip_layout = QtWidgets.QHBoxLayout()
            clip_layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignLeft)
            track_frame.setLayout(clip_layout)

            clip = QtWidgets.QLabel("Clip")
            clip.setFixedSize(60, 40)
            clip.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
            clip.setStyleSheet("""
                QLabel {
                    background-color: #4caf50;
                    color: white;
                    border-radius: 5px;
                }
            """)
            clip_layout.addWidget(clip)

            track_layout.addWidget(track_frame)

        main_layout.addLayout(track_layout)
