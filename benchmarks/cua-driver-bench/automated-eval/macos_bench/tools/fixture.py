#!/usr/bin/env python3
"""Fixture lifecycle for the MB-* tasks: setup, check, reset.

The runner may use this module or reimplement the same three steps (they are the same as run_pilot.py's
launch_lab / evaluate_probe). It is also what validate.py uses.

  fixture.py setup --task MB-01 --seed 5 --dir D [--build-dir B]   start BenchLab, write D/brief.txt
  fixture.py check --task MB-01 --seed 5 --dir D [--sentinel P]    run the evaluator, print the result
  fixture.py reset --task MB-01 --dir D                            kill apps, delete state

All state lives under D (state, events, home). Nothing is read from or written to the user's home.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
PROBES = ROOT / "probes"
DEFAULT_BUILD = ROOT.parent.parent / "build"


def task_meta(task: str) -> dict[str, Any]:
    return json.loads((PROBES / task / "task.json").read_text("utf-8"))


def render(task: str, seed: int) -> str:
    spec = importlib.util.spec_from_file_location("r_" + task, PROBES / task / "render_brief.py")
    assert spec and spec.loader
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m.render(seed)


def paths(d: Path) -> dict[str, Path]:
    return {
        "state": d / "lab-state.json",
        "events": d / "lab-events.jsonl",
        "result": d / "evaluator-result.json",
        "pid": d / "lab.pid",
    }


def sweep(pattern: str) -> None:
    subprocess.run(["pkill", "-9", "-f", pattern], capture_output=True)


def setup(task: str, seed: int, d: Path, build: Path = DEFAULT_BUILD) -> dict[str, Any]:
    t0 = time.monotonic()
    d.mkdir(parents=True, exist_ok=True)
    p = paths(d)
    for k in ("state", "events", "result"):
        p[k].unlink(missing_ok=True)
    home = d / "home"
    home.mkdir(exist_ok=True)
    meta = task_meta(task)
    (d / "brief.txt").write_text(render(task, seed), "utf-8")
    exe = next((build / "BenchLab.app" / "Contents" / "MacOS").iterdir())
    env = {"HOME": str(home), "PATH": "/usr/bin:/bin", "TMPDIR": str(d)}
    proc = subprocess.Popen(
        [
            str(exe),
            *meta["app_args"],
            "--seed",
            str(seed),
            "--state",
            str(p["state"]),
            "--events",
            str(p["events"]),
        ],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    p["pid"].write_text(str(proc.pid))
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and not p["state"].exists():
        time.sleep(0.1)
    time.sleep(0.8)  # window on screen
    return {
        "pid": proc.pid,
        "seconds": round(time.monotonic() - t0, 2),
        "state": str(p["state"]),
        "events": str(p["events"]),
    }


def check(task: str, seed: int, d: Path, sentinel: str | None = None) -> dict[str, Any]:
    t0 = time.monotonic()
    p = paths(d)
    cmd = [
        sys.executable,
        str(PROBES / task / "evaluate.py"),
        "--seed",
        str(seed),
        "--state",
        str(p["state"]),
        "--events",
        str(p["events"]),
        "--result",
        str(p["result"]),
    ]
    if sentinel:
        cmd += ["--sentinel", sentinel]
    subprocess.run(cmd, capture_output=True, text=True, timeout=60)
    res = json.loads(p["result"].read_text("utf-8"))
    res["check_seconds"] = round(time.monotonic() - t0, 3)
    return res


def reset(task: str, d: Path) -> dict[str, Any]:
    t0 = time.monotonic()
    p = paths(d)
    if p["pid"].exists():
        try:
            os.kill(int(p["pid"].read_text()), signal.SIGKILL)
        except (OSError, ValueError):
            pass
    sweep("BenchLab.app/Contents/MacOS/BenchLab")
    meta = task_meta(task)
    for app in meta.get("needs_apps", []):
        sweep(f"/{app}.app/Contents/MacOS/{app}")
    if "Calculator" in meta.get("needs_apps", []):
        # Calculator keeps its display and history in saved state; remove it so every trial starts blank.
        for path in (
            Path.home() / "Library/Saved Application State/com.apple.calculator.savedState",
        ):
            subprocess.run(["rm", "-rf", str(path)], capture_output=True)
        subprocess.run("pbcopy </dev/null", shell=True)
    for k in ("state", "events", "result", "pid"):
        p[k].unlink(missing_ok=True)
    subprocess.run(["rm", "-rf", str(d / "home")], capture_output=True)
    return {"seconds": round(time.monotonic() - t0, 2)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["setup", "check", "reset"])
    ap.add_argument("--task", required=True)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--dir", required=True)
    ap.add_argument("--build-dir", default=str(DEFAULT_BUILD))
    ap.add_argument("--sentinel", default=None)
    a = ap.parse_args()
    d = Path(a.dir)
    if a.cmd == "setup":
        out = setup(a.task, a.seed, d, Path(a.build_dir))
    elif a.cmd == "check":
        out = check(a.task, a.seed, d, a.sentinel)
    else:
        out = reset(a.task, d)
    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
