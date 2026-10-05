"""Summarize a BenchSentinel JSONL log over its armed window.

The armed window runs from the first {"ev":"armed"} line to the next
{"ev":"disarmed"} line, or to EOF if the log is never disarmed. Lines outside
the window are ignored.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from typing import Any

SENTINEL_BID = "ai.cua.benchsentinel"
DEVIATION_PX = 10.0
HID_KEYS = ("move", "down", "key", "scroll")
FRONT_EVENTS = ("s", "front", "armed", "disarmed")
# Idle values only ever grow between samples unless a real event resets them.
DROP_EPSILON_S = 1e-3


def _read_lines(log_path: str) -> list[dict[str, Any]]:
    lines: list[dict[str, Any]] = []
    with open(log_path, encoding="utf-8") as handle:
        for raw in handle:
            raw = raw.strip()
            if not raw:
                continue
            try:
                obj = json.loads(raw)
            except json.JSONDecodeError:
                continue  # a torn final line is not an error
            if isinstance(obj, dict) and isinstance(obj.get("t"), (int, float)):
                lines.append(obj)
    return lines


def _armed_window(lines: list[dict[str, Any]]) -> list[dict[str, Any]]:
    start = next((i for i, ln in enumerate(lines) if ln.get("ev") == "armed"), None)
    if start is None:
        return []
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].get("ev") == "disarmed":
            end = i + 1  # keep the disarm line as the last baseline
            break
    return lines[start:end]


def _bid(front: Any) -> str | None:
    if not isinstance(front, dict):
        return None
    bid = front.get("bid")
    if isinstance(bid, str) and bid:
        return bid
    pid = front.get("pid")
    return f"pid:{pid}" if isinstance(pid, int) and pid >= 0 else None


def _point(value: Any) -> tuple[float, float] | None:
    if (
        isinstance(value, (list, tuple))
        and len(value) == 2
        and all(isinstance(v, (int, float)) for v in value)
    ):
        return float(value[0]), float(value[1])
    return None


def summarize(log_path: str) -> dict[str, Any]:
    """Return disturbance metrics for the armed window of a sentinel log."""
    lines = _read_lines(log_path)
    window = _armed_window(lines)
    result: dict[str, Any] = {
        "front_changes": 0,
        "front_changed_to": [],
        "key_loss": 0,
        "activations_lost": 0,
        "keystrokes_leaked": 0,
        "clicks_leaked": 0,
        "scrolls_leaked": 0,
        "pointer_max_deviation_px": 0.0,
        "pointer_deviation_episodes": 0,
        "hid_events": {k: 0 for k in HID_KEYS},
        "samples": 0,
        "duration_s": 0.0,
        "available": False,
    }
    if not window:
        return result

    armed_line = window[0]
    last = window[-1]
    # A closing "disarmed" line is a boundary, not part of the measured events.
    body = window[:-1] if last.get("ev") == "disarmed" and len(window) > 1 else window
    result["duration_s"] = round((last["t"] - armed_line["t"]) / 1000.0, 3)

    origin = _point(armed_line.get("mouse"))
    prev_front = _bid(armed_line.get("front"))
    prev_idle: dict[str, float] | None = None
    changed_to: set[str] = set()
    in_episode = False

    for line in window:
        # Front changes come from samples and workspace activation events only;
        # other events read the frontmost app and can be stale at that instant.
        front = _bid(line.get("front")) if line.get("ev") in FRONT_EVENTS else None
        if front is not None:
            if front != prev_front and front != SENTINEL_BID:
                result["front_changes"] += 1
                changed_to.add(front)
            prev_front = front

        # HID idle drops: baseline is the armed line, then each sample.
        idle = line.get("idle")
        if isinstance(idle, dict) and line.get("ev") in ("s", "armed", "disarmed"):
            current = {k: float(idle[k]) for k in HID_KEYS if isinstance(idle.get(k), (int, float))}
            if prev_idle is not None and line is not armed_line:
                for k in HID_KEYS:
                    if (
                        k in current
                        and k in prev_idle
                        and current[k] < prev_idle[k] - DROP_EPSILON_S
                    ):
                        result["hid_events"][k] += 1
            prev_idle = current

    for line in body:
        ev = line.get("ev")
        if ev == "didResignKey":
            result["key_loss"] += 1
        elif ev == "didResignActive":
            result["activations_lost"] += 1
        elif ev == "keyDown":
            result["keystrokes_leaked"] += 1
        elif ev == "mouseDown":
            result["clicks_leaked"] += 1
        elif ev == "scrollWheel":
            result["scrolls_leaked"] += 1
        elif ev == "s":
            result["samples"] += 1
            point = _point(line.get("mouse"))
            if origin is None and point is not None:
                origin = point
            if point is not None and origin is not None:
                dist = math.hypot(point[0] - origin[0], point[1] - origin[1])
                result["pointer_max_deviation_px"] = max(result["pointer_max_deviation_px"], dist)
                if dist > DEVIATION_PX:
                    if not in_episode:
                        result["pointer_deviation_episodes"] += 1
                    in_episode = True
                else:
                    in_episode = False

    result["pointer_max_deviation_px"] = round(result["pointer_max_deviation_px"], 2)
    result["front_changed_to"] = sorted(changed_to)
    result["available"] = result["samples"] >= 10
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", help="BenchSentinel JSONL log")
    args = parser.parse_args(argv)
    json.dump(summarize(args.log), sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
