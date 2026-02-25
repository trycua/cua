"""Trajectory recording utility for cua do actions.

Records each action into a replayable trajectory compatible with the
TrajectoryViewer at cua.ai/trajectory-viewer.

All state is file-based (each `cua do` invocation is a separate process).
The current session path is stored in ~/.cua/do_target.json under the
``trajectory_session`` key.

Directory layout::

    ~/.cua/trajectories/{machine_name}/{YYYYMMDD-HHMMSS}/
      turn_001/
        screenshot.png
        turn_001_agent_response.json
      turn_002/
        screenshot.png
        turn_002_agent_response.json
"""

from __future__ import annotations

import json
import shutil
import time
import uuid
import zipfile
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

_CUA_DIR = Path.home() / ".cua"
_TRAJECTORIES_DIR = _CUA_DIR / "trajectories"
_STATE_FILE = _CUA_DIR / "do_target.json"


# ── state helpers ────────────────────────────────────────────────────────────


def _load_state() -> dict:
    if _STATE_FILE.exists():
        try:
            return json.loads(_STATE_FILE.read_text())
        except Exception:
            pass
    return {}


def _save_state(state: dict) -> None:
    _STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
    _STATE_FILE.write_text(json.dumps(state, indent=2))


# ── session management ───────────────────────────────────────────────────────


def ensure_session(state: dict) -> Path:
    """Return the current session directory, creating one if needed.

    If ``trajectory_session`` is already set in *state* and the directory
    exists, it is reused.  Otherwise a new timestamped directory is created
    under ``~/.cua/trajectories/{machine_name}/`` and persisted to state.
    """
    existing = state.get("trajectory_session")
    if existing:
        p = Path(existing)
        if p.is_dir():
            return p

    machine = state.get("name") or state.get("provider") or "unknown"
    ts = datetime.now().strftime("%Y%m%d-%H%M%S")
    session_dir = _TRAJECTORIES_DIR / machine / ts
    session_dir.mkdir(parents=True, exist_ok=True)

    state["trajectory_session"] = str(session_dir)
    _save_state(state)
    return session_dir


def reset_session(state: dict) -> None:
    """Clear ``trajectory_session`` from state (called on ``cua do switch``)."""
    state.pop("trajectory_session", None)
    _save_state(state)


# ── turn recording ───────────────────────────────────────────────────────────


def get_next_turn_number(session_dir: Path) -> int:
    """Scan for existing ``turn_NNN`` dirs and return the next number."""
    max_n = 0
    for child in session_dir.iterdir():
        if child.is_dir() and child.name.startswith("turn_"):
            try:
                n = int(child.name.split("_", 1)[1])
                if n > max_n:
                    max_n = n
            except (ValueError, IndexError):
                pass
    return max_n + 1


def _build_action_dict(action_type: str, action_params: dict[str, Any]) -> dict[str, Any]:
    """Build the ``action`` dict for the agent-response JSON."""
    action: dict[str, Any] = {"type": action_type}
    action.update(action_params)
    return action


def record_turn(
    session_dir: Path,
    action_type: str,
    action_params: dict[str, Any],
    screenshot_bytes: bytes | None = None,
) -> Path:
    """Write a single turn to the session directory.

    Creates ``turn_NNN/screenshot.png`` (if *screenshot_bytes* provided)
    and ``turn_NNN/turn_NNN_agent_response.json``.

    Returns the turn directory.
    """
    turn_num = get_next_turn_number(session_dir)
    turn_name = f"turn_{turn_num:03d}"
    turn_dir = session_dir / turn_name
    turn_dir.mkdir(parents=True, exist_ok=True)

    # Screenshot
    if screenshot_bytes:
        (turn_dir / "screenshot.png").write_bytes(screenshot_bytes)

    # Agent response JSON (TrajectoryViewer-compatible)
    call_id = f"call_{uuid.uuid4().hex[:12]}"
    ts_ms = int(time.time())

    response_json: dict[str, Any] = {
        "model": "cua-cli",
        "response": {
            "id": f"resp_{int(time.time() * 1000)}",
            "object": "response",
            "created_at": ts_ms,
            "status": "completed",
            "model": "cua-cli",
            "output": [
                {
                    "type": "computer_call",
                    "id": call_id,
                    "call_id": call_id,
                    "action": _build_action_dict(action_type, action_params),
                    "pending_safety_checks": [],
                    "status": "completed",
                }
            ],
        },
    }

    json_path = turn_dir / f"{turn_name}_agent_response.json"
    json_path.write_text(json.dumps(response_json, indent=2))

    return turn_dir


# ── listing / inspection ─────────────────────────────────────────────────────


def list_trajectories(machine: str | None = None) -> list[dict[str, Any]]:
    """Return a list of trajectory sessions.

    Each entry is ``{machine, session, path, turns, created}``.
    If *machine* is given, only that machine's sessions are returned.
    """
    results: list[dict[str, Any]] = []
    if not _TRAJECTORIES_DIR.is_dir():
        return results

    machines = [_TRAJECTORIES_DIR / machine] if machine else sorted(_TRAJECTORIES_DIR.iterdir())

    for machine_dir in machines:
        if not machine_dir.is_dir():
            continue
        for session_dir in sorted(machine_dir.iterdir()):
            if not session_dir.is_dir():
                continue
            turns = sum(
                1 for c in session_dir.iterdir() if c.is_dir() and c.name.startswith("turn_")
            )
            # Parse created timestamp from dir name
            try:
                created = datetime.strptime(session_dir.name, "%Y%m%d-%H%M%S")
            except ValueError:
                created = datetime.fromtimestamp(session_dir.stat().st_ctime)
            results.append(
                {
                    "machine": machine_dir.name,
                    "session": session_dir.name,
                    "path": str(session_dir),
                    "turns": turns,
                    "created": created.isoformat(),
                }
            )
    return results


# ── zipping ──────────────────────────────────────────────────────────────────


def zip_trajectory(session_path: str | Path) -> Path:
    """Create a zip of a session directory in TrajectoryViewer-compatible format.

    Returns the path to the created zip file (placed next to the session dir).
    """
    session_dir = Path(session_path)
    zip_path = session_dir.parent / f"{session_dir.name}.zip"
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for child in sorted(session_dir.rglob("*")):
            if child.is_file():
                zf.write(child, child.relative_to(session_dir))
    return zip_path


# ── cleanup ──────────────────────────────────────────────────────────────────


def clean_trajectories(
    older_than_days: int | None = None,
    machine: str | None = None,
) -> list[str]:
    """Delete trajectory sessions matching the criteria.

    Returns a list of deleted session paths.
    """
    deleted: list[str] = []
    if not _TRAJECTORIES_DIR.is_dir():
        return deleted

    cutoff = datetime.now() - timedelta(days=older_than_days) if older_than_days else None
    machines = [_TRAJECTORIES_DIR / machine] if machine else sorted(_TRAJECTORIES_DIR.iterdir())

    for machine_dir in machines:
        if not machine_dir.is_dir():
            continue
        for session_dir in sorted(machine_dir.iterdir()):
            if not session_dir.is_dir():
                continue
            if cutoff:
                try:
                    created = datetime.strptime(session_dir.name, "%Y%m%d-%H%M%S")
                except ValueError:
                    created = datetime.fromtimestamp(session_dir.stat().st_ctime)
                if created >= cutoff:
                    continue
            shutil.rmtree(session_dir)
            deleted.append(str(session_dir))
            # Also remove zip if it exists
            zip_path = session_dir.parent / f"{session_dir.name}.zip"
            if zip_path.exists():
                zip_path.unlink()
        # Remove empty machine dirs
        if machine_dir.is_dir() and not any(machine_dir.iterdir()):
            machine_dir.rmdir()

    return deleted
