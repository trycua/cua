"""Optional progress reports for the pool's stall watchdog.

You do not need this: while your function runs, the runtime counts new output
and CPU time as progress.  Call `cua.progress.step(n)` from your training loop
when you want the watchdog to follow your steps exactly instead (it also shows
the step in `cua jobs`); from then on output and CPU time no longer count in
this process.
"""
from __future__ import annotations

import json
import os
import threading
import time
from pathlib import Path

__all__ = ["step"]

_lock = threading.Lock()
_last = 0.0
explicit = False  # read by the runtime: the user reports steps themselves


def write(step_n: int, force: bool = False) -> bool:
    """Write the heartbeat file (CUA_HEARTBEAT) naming `step_n`.  At most once
    a second unless `force`.  No-op outside the pool."""
    global _last
    path = os.environ.get("CUA_HEARTBEAT")
    if not path:
        return False
    now = time.time()
    with _lock:
        if not force and now - _last < 1.0:
            return False
        _last = now
    doc = {"t_ms": int(now * 1000), "step": int(step_n),
           "rank": int(os.environ.get("CUA_RANK", "0") or 0),
           "attempt": int(os.environ.get("CUA_ATTEMPT", "1") or 1)}
    p = Path(path)
    tmp = p.with_name(f"{p.name}.{os.getpid()}.tmp")
    try:
        tmp.write_text(json.dumps(doc))
        os.replace(tmp, p)
    except OSError:
        return False  # liveness reporting must never break training
    return True


def step(n: int) -> None:
    """Report that training reached step `n` (cheap; call it every step)."""
    global explicit
    explicit = True
    write(n)
