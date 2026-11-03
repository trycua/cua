"""Inside a bench task's computer, as its session user: arm the bench's sentinel just before the agent starts, or disarm
it just after, as the bench's runner does. Arming empties the clipboard and activates the sentinel until a sample says
its window is active and key, then arms it, and fails unless the armed line says so too and, with ``condition``, unless
the sentinel runs in that condition. Each waits for the sentinel's log to say it's done, and does nothing when the
sentinel is already in that state; disarming a sentinel that's no longer running says so, for the grade to fail the
trial. ``args``: ``action``, ``arm`` or ``disarm``, and ``condition``. Returns the log's armed or disarmed line.

It is the macOS counterpart of the sentinel's arming step on Linux: the app is BenchSentinel, which takes SIGUSR1 to
activate and SIGUSR2 to toggle armed, and logs one JSON line per event."""

import json
import os
import signal
import subprocess
import time
from pathlib import Path

LOG = Path.home() / ".bench-sentinel" / "sentinel.jsonl"
WAIT_S = 30.0
ACTIVATE_EVERY_S = 3.0


def run(input):
    action = input["args"]["action"]
    if action not in ("arm", "disarm"):
        raise ValueError(f"action is arm or disarm, not {action!r}")
    log = Log(LOG)
    pid, armed, condition = log.state()
    wanted = input["args"].get("condition")
    if wanted not in (None, condition):
        raise RuntimeError(f"the sentinel runs in the {condition} condition, not {wanted}")
    if armed == (action == "arm"):
        return {"action": action, "already": True, "line": log.last(("armed", "disarmed"))}
    if action == "arm":
        subprocess.run(["pbcopy"], input=b"", check=True)
        log.activate(pid)
    try:
        os.kill(pid, signal.SIGUSR2)
    except ProcessLookupError:
        if action == "arm":
            raise
        return {"action": action, "already": False, "line": None, "sentinel": "gone"}
    line = log.wait(lambda line: line["ev"] == ("armed" if action == "arm" else "disarmed"))
    if action == "arm" and not (line["active"] and line["key"]):
        raise RuntimeError(f"the sentinel armed without its window active and key: {line}")
    return {"action": action, "already": False, "line": line}


class Log:
    """The sentinel's log, read on from where it was last read."""

    def __init__(self, path):
        self.path, self.offset = path, 0

    def state(self):
        """The running sentinel's pid, whether it's armed and its condition, waiting for it to start."""
        deadline = time.monotonic() + WAIT_S
        while True:
            pid, armed = None, False
            for line in self.read():
                if line["ev"] == "start":
                    pid, armed = line["pid"], False
                elif line["ev"] in ("armed", "disarmed"):
                    armed = line["ev"] == "armed"
            if pid is not None:
                return pid, armed, _condition(pid)
            if time.monotonic() > deadline:
                raise RuntimeError(f"the sentinel hasn't started: {self.path} has no start line")
            self.offset = 0
            time.sleep(0.5)

    def last(self, events):
        self.offset = 0
        return next((line for line in reversed(self.read()) if line["ev"] in events), None)

    def activate(self, pid):
        """Activate the sentinel until a sample says its window is active and key."""
        deadline, sent = time.monotonic() + WAIT_S, 0.0
        while time.monotonic() < deadline:
            if time.monotonic() - sent > ACTIVATE_EVERY_S:
                os.kill(pid, signal.SIGUSR1)
                sent = time.monotonic()
            if any(line["ev"] == "s" and line["active"] and line["key"] for line in self.read()):
                return
            time.sleep(0.2)
        raise RuntimeError("the sentinel's window didn't become active and key")

    def wait(self, wanted):
        deadline = time.monotonic() + WAIT_S
        while time.monotonic() < deadline:
            line = next((line for line in self.read() if wanted(line)), None)
            if line is not None:
                return line
            time.sleep(0.2)
        raise RuntimeError("the sentinel didn't answer its signal")

    def read(self):
        """The whole lines written since the last read; a torn last line is left for the next."""
        if not self.path.exists():
            return []
        with open(self.path, "rb") as log:
            log.seek(self.offset)
            data = log.read()
        whole = data[: data.rfind(b"\n") + 1]
        self.offset += len(whole)
        return [json.loads(raw) for raw in whole.splitlines() if raw.strip()]


def _condition(pid: int) -> str:
    """The running sentinel's condition, from its command line: ``--user-activity`` is the background condition."""
    done = subprocess.run(["ps", "-o", "command=", "-p", str(pid)], capture_output=True, text=True)
    return "background" if "--user-activity" in done.stdout else "foreground"
