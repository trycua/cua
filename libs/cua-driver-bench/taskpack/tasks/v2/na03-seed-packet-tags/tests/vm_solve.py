"""Scripted solution for NA-03, used only by `_kit/devtools/vm_validate.py` in a macOS VM.

It changes the app's data or files directly (not through the GUI), to prove the evaluator passes a solved state.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_kit"))

from v2kit import finder  # noqa: E402


def solve(workspace: Path) -> None:
    folder = workspace / "Seed packets 2026"
    for n in ["tomato-amberlight.txt", "tomato-red-ember.txt", "squash-moonbowl.txt", "squash-ridgeback.txt"]:
        finder.set_tags(folder / n, ["Red"])
    for n in ["bean-cobble-runner.txt", "bean-violet-pod.txt", "pea-tallow-sugar.txt"]:
        finder.set_tags(folder / n, ["Green"])
    finder.set_tags(folder / "lettuce-frill-mint.txt", ["Blue"])
