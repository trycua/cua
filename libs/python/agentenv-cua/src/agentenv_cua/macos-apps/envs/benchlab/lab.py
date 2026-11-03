"""The app's launch: BenchLab as the bench's runner starts it, the executable of ``BenchLab.app`` in the mode and with
the seed of the app's settings and a throwaway home, writing its state and events into the app's data folder. The
process becomes BenchLab, so the app server's launch is BenchLab and stopping the one stops the other."""

import os
import tempfile
from pathlib import Path

from app import DATA, BenchLab

HERE = Path(__file__).resolve().parent
EXECUTABLE = HERE / "build" / "BenchLab.app" / "Contents" / "MacOS" / "BenchLab"


def argv(settings, data: Path) -> list[str]:
    return [
        str(EXECUTABLE),
        "--mode",
        settings.mode,
        "--seed",
        str(settings.seed),
        "--state",
        str(data / "state.json"),
        "--events",
        str(data / "events.jsonl"),
    ]


def main(settings) -> None:
    data = Path.home() / DATA
    data.mkdir(exist_ok=True)
    home = tempfile.mkdtemp(prefix="benchlab-home-")
    os.execve(str(EXECUTABLE), argv(settings, data), {**os.environ, "HOME": home})


if __name__ == "__main__":
    main(BenchLab().settings)
