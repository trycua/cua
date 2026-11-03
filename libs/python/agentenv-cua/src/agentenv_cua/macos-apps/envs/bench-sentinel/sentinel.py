"""The app's launch: BenchSentinel as the bench's runner starts it, ``BenchSentinel --log PATH``, with the background
condition's ``--user-activity --frame`` when the app's settings say so. The process becomes BenchSentinel, so the app
server's launch is the sentinel: SIGUSR1 activates it, SIGUSR2 arms or disarms it, SIGTERM stops it.

``sentinel.py --ready`` is the app's ready check: it exits 0 once the log has a ``start`` line and a sample."""

import json
import os
import sys
from pathlib import Path

from app import LOG_FOLDER, USER_FRAME, BenchSentinel

HERE = Path(__file__).resolve().parent
EXECUTABLE = HERE / "build" / "BenchSentinel.app" / "Contents" / "MacOS" / "BenchSentinel"
LOG = Path.home() / LOG_FOLDER / "sentinel.jsonl"


def argv(condition: str, log: Path = LOG) -> list[str]:
    args = [str(EXECUTABLE), "--log", str(log)]
    if condition == "background":
        args += ["--user-activity", "--frame", ",".join(str(v) for v in USER_FRAME)]
    return args


def ready(log: Path = LOG) -> bool:
    """Whether the log shows the sentinel started and sampling."""
    try:
        lines = log.read_text(encoding="utf-8").splitlines()
    except OSError:
        return False
    events = set()
    for line in lines:
        try:
            events.add(json.loads(line)["ev"])
        except (ValueError, KeyError, TypeError):
            continue  # a torn last line
    return {"start", "s"} <= events


def main() -> None:
    if sys.argv[1:] == ["--ready"]:
        sys.exit(0 if ready() else 1)
    LOG.parent.mkdir(exist_ok=True)
    os.execv(str(EXECUTABLE), argv(BenchSentinel().settings.condition))


if __name__ == "__main__":
    main()
