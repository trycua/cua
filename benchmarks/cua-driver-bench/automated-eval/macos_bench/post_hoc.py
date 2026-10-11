#!/usr/bin/env python3
"""Post-hoc disturbance attribution (descriptive, added after the first trials ran).

The pre-registered disturbance counters come from BenchSentinel. Two things they cannot tell
apart are tool-caused and human-caused effects, and how long keyboard focus stayed away. This
script reads each trial's sentinel log and Codex event log and reports:

* activations_lost: times the frontmost app resigned active (a flicker even when it stays frontmost);
* hid_any_drops: real HID input events seen by the 1 Hz ioreg sampler (human or HID-level synthetic input);
* key_lost_s: seconds the sentinel window was not the key window while armed;
* key_lost_during_tool_s: the part of that time inside a tool-call window;
* pointer_moves_total / pointer_moves_during_tool: sampled pointer displacements (> 2 px) and
  how many fall inside a tool-call window (start - 0.3 s .. end + 1.5 s);
* pointer_max_dev_during_tool_px: largest distance from the armed position seen inside tool windows.

Unattributed pointer movement (outside any tool window) is most likely a human at the machine.
Nothing here changes pass/fail or the pre-registered counters.

usage: post_hoc.py RUN_DIR [--out FILE]
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import codex_events  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parent / "swift"))
import summarize_sentinel  # noqa: E402


def tool_windows(events: list[tuple[float, dict]]) -> list[tuple[float, float]]:
    started: dict[str, float] = {}
    windows: list[tuple[float, float]] = []
    for stamp, event in events:
        item = event.get("item") or {}
        if item.get("type") not in ("mcp_tool_call", "command_execution"):
            continue
        key = str(item.get("id"))
        if event.get("type") == "item.started":
            started[key] = stamp
        elif event.get("type") == "item.completed":
            begin = started.pop(key, stamp)
            if item.get("type") == "command_execution" and "cua-driver" not in str(
                item.get("command")
            ):
                continue
            if (
                item.get("type") == "mcp_tool_call"
                and item.get("tool") in codex_events.CODEX_BUILTIN_MCP_TOOLS
            ):
                continue
            windows.append((begin - 300.0, stamp + 1500.0))
    return windows


def inside(t: float, windows: list[tuple[float, float]]) -> bool:
    return any(a <= t <= b for a, b in windows)


def analyse(trial: Path) -> dict | None:
    log = trial / "artifacts" / "sentinel.jsonl"
    events_path = trial / "codex-events.tsv"
    if not log.exists() or not events_path.exists():
        return None
    windows = tool_windows(codex_events.read_events(events_path))
    armed = False
    samples: list[dict] = []
    for line in log.read_text("utf-8", errors="replace").splitlines():
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("ev") == "armed":
            armed = True
        elif row.get("ev") == "disarmed":
            armed = False
        if armed and row.get("ev") == "s":
            samples.append(row)
    if len(samples) < 2:
        return None
    origin = samples[0]["mouse"]
    key_lost = key_lost_tool = 0.0
    moves = moves_tool = 0
    dev_tool = 0.0
    previous = samples[0]
    for sample in samples[1:]:
        dt = (sample["t"] - previous["t"]) / 1000.0
        if previous.get("key") is False:
            key_lost += dt
            if inside(previous["t"], windows):
                key_lost_tool += dt
        step = math.dist(previous["mouse"], sample["mouse"])
        if step > 2:
            moves += 1
            if inside(sample["t"], windows):
                moves_tool += 1
        if inside(sample["t"], windows):
            dev_tool = max(dev_tool, math.dist(origin, sample["mouse"]))
        previous = sample
    counters = summarize_sentinel.summarize(str(log))
    idle_log = trial / "artifacts" / "hid-idle.jsonl"
    hid_any = None
    if idle_log.exists():
        values = [json.loads(line)["idle_s"] for line in idle_log.read_text("utf-8").splitlines()]
        hid_any = sum(1 for a, b in zip(values, values[1:]) if b + 0.5 < a)
    return {
        "trial_id": trial.name,
        "activations_lost": counters.get("activations_lost"),
        "key_loss_events": counters.get("key_loss"),
        "hid_any_drops": hid_any,
        "key_lost_s": round(key_lost, 2),
        "key_lost_during_tool_s": round(key_lost_tool, 2),
        "pointer_moves_total": moves,
        "pointer_moves_during_tool": moves_tool,
        "pointer_max_dev_during_tool_px": round(dev_tool, 1),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("run_dir", type=Path)
    parser.add_argument("--out", type=Path, default=None)
    args = parser.parse_args()
    rows = [r for t in sorted((args.run_dir / "trials").iterdir()) if (r := analyse(t))]
    text = "\n".join(json.dumps(r) for r in rows) + "\n"
    if args.out:
        args.out.write_text(text, "utf-8")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
