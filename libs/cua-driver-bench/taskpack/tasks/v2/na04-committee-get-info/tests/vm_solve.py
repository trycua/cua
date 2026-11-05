"""Scripted solution for NA-04, used only by `_kit/devtools/vm_validate.py` in a macOS VM.

It changes the app's data or files directly (not through the GUI), to prove the evaluator passes a solved state.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_kit"))

import subprocess  # noqa: E402

from v2kit import finder  # noqa: E402


def solve(workspace: Path) -> None:
    folder = workspace / "Committee papers"
    subprocess.run(["/usr/bin/chflags", "uchg", str(folder / "constitution-2024.txt")], check=True)
    finder.set_comment(folder / "minutes-march.txt", "Approved at the April meeting")
    (folder / "draft agenda.txt").rename(folder / "agenda-may.txt")
