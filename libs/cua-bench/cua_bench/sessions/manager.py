"""Run/session bookkeeping for ``cb run`` (``$XDG_STATE_HOME/cua-bench/runs.json``).

Parallel variants and detached runs update the same file, so every
read-modify-write holds an exclusive lock.
"""

import contextlib
import json
import os
import time
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional

try:
    import fcntl
except ImportError:  # Windows
    fcntl = None  # type: ignore[assignment]


def _get_state_dir() -> Path:
    """Get XDG state directory for cua-bench."""
    xdg_state = os.environ.get("XDG_STATE_HOME", os.path.expanduser("~/.local/state"))
    return Path(xdg_state) / "cua-bench"


# Session storage path
RUNS_FILE = _get_state_dir() / "runs.json"


def _load_runs() -> Dict[str, Any]:
    """Load runs from the storage file."""
    if not RUNS_FILE.exists():
        return {}

    try:
        with open(RUNS_FILE, "r") as f:
            return json.load(f)
    except (json.JSONDecodeError, IOError):
        return {}


def _save_runs(runs: Dict[str, Any]) -> None:
    """Save runs to the storage file (atomically)."""
    RUNS_FILE.parent.mkdir(parents=True, exist_ok=True)
    tmp = RUNS_FILE.with_suffix(f".{os.getpid()}.tmp")
    with open(tmp, "w") as f:
        json.dump(runs, f, indent=2)
    os.replace(tmp, RUNS_FILE)


@contextlib.contextmanager
def _locked() -> Iterator[None]:
    RUNS_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(RUNS_FILE.with_suffix(".lock"), "a") as handle:
        if fcntl is not None:
            fcntl.flock(handle, fcntl.LOCK_EX)
        try:
            yield
        finally:
            if fcntl is not None:
                fcntl.flock(handle, fcntl.LOCK_UN)


def add_session(session_data: Dict[str, Any]) -> None:
    """Add a new session to the storage.

    Args:
        session_data: Session metadata dict
    """
    session_id = session_data["session_id"]

    # Add timestamp if not present
    if "created_at" not in session_data:
        session_data["created_at"] = time.time()

    with _locked():
        runs = _load_runs()
        runs[session_id] = session_data
        _save_runs(runs)


def remove_session(session_id: str) -> None:
    """Remove a session from storage.

    Args:
        session_id: Session identifier
    """
    with _locked():
        runs = _load_runs()
        if session_id in runs:
            del runs[session_id]
            _save_runs(runs)


def update_session(session_id: str, updates: Dict[str, Any]) -> None:
    """Update session metadata.

    Args:
        session_id: Session identifier
        updates: Dict of fields to update
    """
    with _locked():
        runs = _load_runs()
        if session_id in runs:
            runs[session_id].update(updates)
            _save_runs(runs)


def list_sessions(provider: Optional[str] = None) -> List[Dict[str, Any]]:
    """List all stored sessions.

    Args:
        provider: Optional provider filter ("docker", "cua-cloud", etc.)

    Returns:
        List of session metadata dicts
    """
    runs = _load_runs()
    session_list = list(runs.values())

    # Filter by provider if specified
    if provider:
        session_list = [s for s in session_list if s.get("provider") == provider]

    # Sort by creation time (newest first)
    session_list.sort(key=lambda s: s.get("created_at", 0), reverse=True)

    return session_list


def get_session(session_id: str) -> Optional[Dict[str, Any]]:
    """Get session metadata by ID.

    Args:
        session_id: Session identifier

    Returns:
        Session metadata dict or None if not found
    """
    runs = _load_runs()
    return runs.get(session_id)
