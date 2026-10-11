"""Status of a recorded run variant, without asking any container runtime.

The runner records ``status`` (queued, starting, running, completed, failed,
cancelled) and ``reward`` in ``runs.json`` and writes ``result.json`` next to
the variant's ``run.log``. A variant left ``running`` by a process that no
longer exists is reported from its ``result.json``, or as ``failed``.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Dict, Optional

FINAL = ("completed", "failed", "cancelled", "stopped")


def _pid_alive(pid: Any) -> bool:
    try:
        pid = int(pid)
    except (TypeError, ValueError):
        return False
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


def read_result(session: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    output_dir = session.get("output_dir")
    if not output_dir:
        return None
    try:
        return json.loads((Path(output_dir) / "result.json").read_text())
    except (OSError, ValueError):
        return None


def session_status(session: Dict[str, Any]) -> Dict[str, Any]:
    """``{"status": ..., "reward": ...}`` for a runs.json entry."""
    status = session.get("status") or "unknown"
    reward = session.get("reward")
    if status in ("starting", "running"):
        if not _pid_alive(session.get("pid")):
            result = read_result(session)
            if result is not None:
                status = result.get("status") or "failed"
                reward = result.get("reward", reward)
            else:
                status = "failed"
    if status == "failed" and reward is None:
        reward = 0.0
    return {"session_id": session.get("session_id"), "status": status, "reward": reward}


def session_logs(session: Dict[str, Any], tail: Optional[int] = None) -> str:
    output_dir = session.get("output_dir")
    if not output_dir:
        return ""
    try:
        text = (Path(output_dir) / "run.log").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    if tail:
        text = "\n".join(text.splitlines()[-tail:])
    return text
