# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""`run_bench.py` with its GUI, daemon and model edges replaced, for the kill-and-resume tests.

Everything from argument parsing to results.jsonl, the lock, the manifest, the retry loop and finalize is the
real runner. Each attempt writes its directory, then sleeps, then returns a row. Environment:

* ``FAKE_PLAN``: JSON ``{trial_id: [infra kind or null per attempt]}``; a missing trial passes first time.
* ``FAKE_KILL``: ``<trial_id>:<attempt>:<point>``. ``agent`` SIGKILLs the runner mid-attempt (directory written,
  no row); ``append`` SIGKILLs it halfway through writing the attempt's row. Only the first run that reaches
  the point dies (a marker in the run directory records it), so the resume runs past it.
* ``FAKE_SLEEP``: seconds each attempt takes (default 0.05).
* ``FAKE_VIDEO_FAILS``: compression leaves a video without its record.
"""

from __future__ import annotations

import json
import os
import signal
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import bench_core as core  # noqa: E402
import run_bench as rb  # noqa: E402

PLAN = json.loads(os.environ.get("FAKE_PLAN") or "{}")
KILL = os.environ.get("FAKE_KILL", "")
SLEEP = float(os.environ.get("FAKE_SLEEP") or 0.05)


def kill_now(run_dir: Path, point: str, trial: str, attempt: int) -> bool:
    marker = run_dir / "fake-killed"
    if KILL != f"{trial}:{attempt}:{point}" or marker.exists():
        return False
    marker.write_text(KILL)
    return True


def fake_attempt(ctx: rb.Ctx, entry: dict, attempt: int) -> dict:
    trial = entry["trial_id"]
    attempt_dir = ctx.run_dir / "trials" / trial / f"a{attempt}"
    (attempt_dir / "artifacts").mkdir(parents=True, exist_ok=True)
    (attempt_dir / "artifacts" / "started").write_text(str(os.getpid()))
    if kill_now(ctx.run_dir, "agent", trial, attempt):
        os.kill(os.getpid(), signal.SIGKILL)
    time.sleep(SLEEP)
    plan = PLAN.get(trial, [])
    kind = plan[attempt - 1] if attempt <= len(plan) else None
    row = {
        "schema": rb.SCHEMA,
        "trial_id": trial,
        "attempt": attempt,
        "final": False,
        **rb.trial_identity(ctx, entry),
        "bench_src_sha256": ctx.versions.get("bench_src_sha256"),
        "run_id": ctx.run_dir.name,
        "status": "infra_error" if kind else "completed",
        "passed": not kind,
        "wall_s": SLEEP,
        "turns": 1,
        "cost_usd": 0.0,
        "infra_failure": kind,
        "infra_message": kind or "",
        "excluded": bool(kind),
        "writer_pid": os.getpid(),
    }
    (attempt_dir / "trial.json").write_text(json.dumps(row) + "\n")
    return row


real_append = core.append_row


def append_or_die(path: Path, row: dict) -> None:
    if kill_now(path.parent, "append", row["trial_id"], row["attempt"]):
        text = json.dumps(row) + "\n"
        with path.open("a", encoding="utf-8") as handle:
            handle.write(text[: len(text) // 2])
        os.kill(os.getpid(), signal.SIGKILL)
    real_append(path, row)


class FailingCompressQueue:
    """FAKE_VIDEO_FAILS=1: the queue ran, but writing a video's record failed, as on a full disk."""

    def __init__(self) -> None:
        self.results = {"/nonexistent/video": {"error": "OSError: No space left on device"}}

    def pending(self) -> int:
        return 0

    def join(self) -> None:
        pass

    def close(self) -> None:
        pass


if os.environ.get("FAKE_VIDEO_FAILS"):
    rb.start_recorder = lambda ctx: setattr(ctx, "compress", FailingCompressQueue())
rb.run_attempt = fake_attempt
rb.setup_arms = lambda ctx: None
rb.preflight = lambda ctx, with_models=True: []
rb.unregister_stale_copies = lambda build_dir: []
rb.stop_agent_daemons = lambda ctx, keep=None: None
rb.ensure_agent_daemon = lambda ctx, arm="cc-cua-driver": None
rb.ca.stop_cua_daemon = lambda *a, **k: None
rb.log_pause = lambda ctx, kind, seconds, message, trial_id: None
core.append_row = append_or_die

if __name__ == "__main__":
    raise SystemExit(rb.main())
