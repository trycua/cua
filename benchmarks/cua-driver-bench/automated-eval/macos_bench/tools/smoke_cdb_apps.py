#!/usr/bin/env python3
"""Start the apps of one CDB task the way the runner does, place the windows, hold for a screenshot, stop.
No model call. Run from Terminal.app inside the benchmark VM.   smoke_cdb_apps.py CDB-S01 [hold_seconds]"""

from __future__ import annotations

import json
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import cdb_adapter  # noqa: E402
import claude_arms as ca  # noqa: E402
import run_bench  # noqa: E402

task_id = sys.argv[1]
hold = float(sys.argv[2]) if len(sys.argv) > 2 else 20
spec = json.loads((HERE.parent / "probes" / task_id / "task.json").read_text("utf-8"))
art = Path(tempfile.mkdtemp(prefix="cdbsmoke-"))
proc = ca.start_cua_daemon(ca.RECORDER_SOCKET, ca.RECORDER_STATE, overlay=False, log_name="smoke.log")
task = cdb_adapter.CdbTask(spec, art)
try:
    task.reset()
    ctx = None
    task.start_apps(windows=lambda win: run_bench.place_window(ctx, win))
    print("apps started; holding", hold, "s", flush=True)
    time.sleep(hold)
finally:
    task.stop_apps()
    task.clean_workspace()
    ca.stop_cua_daemon(ca.RECORDER_SOCKET, ca.RECORDER_STATE / "home")
    proc.terminate()
print("done", art)
