"""Persistent state tracking for local sandboxes.

Each running local sandbox writes a JSON file at ~/.cua/sandboxes/{name}.json.
This allows Sandbox.connect(name, local=True), Sandbox.list(local=True), etc.
to work across Python process restarts.

State files are written by Sandbox.create() for local runtimes and deleted by
Sandbox.delete(). Ephemeral sandboxes never write state files.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

SANDBOX_STATE_DIR = Path.home() / ".cua" / "sandboxes"


def _state_path(name: str) -> Path:
    return SANDBOX_STATE_DIR / f"{name}.json"


def save(
    name: str,
    *,
    runtime_type: str,
    image: dict,
    host: str,
    api_port: int,
    vnc_port: Optional[int] = None,
    qmp_port: Optional[int] = None,
    grpc_port: Optional[int] = None,
    adb_serial: Optional[str] = None,
    sdk_root: Optional[str] = None,
    disk_path: Optional[str] = None,
    os_type: Optional[str] = None,
    vnc_display: Optional[int] = None,
    memory_mb: Optional[int] = None,
    cpu_count: Optional[int] = None,
    arch: Optional[str] = None,
    status: str = "running",
) -> None:
    """Write or overwrite the state file for a local sandbox."""
    SANDBOX_STATE_DIR.mkdir(parents=True, exist_ok=True)
    data: dict[str, Any] = {
        "name": name,
        "runtime_type": runtime_type,
        "image": image,
        "host": host,
        "api_port": api_port,
        "vnc_port": vnc_port,
        "qmp_port": qmp_port,
        "grpc_port": grpc_port,
        "adb_serial": adb_serial,
        "sdk_root": sdk_root,
        "disk_path": disk_path,
        "os_type": os_type,
        "vnc_display": vnc_display,
        "memory_mb": memory_mb,
        "cpu_count": cpu_count,
        "arch": arch,
        "status": status,
        "created_at": datetime.now(timezone.utc).isoformat(),
    }
    _state_path(name).write_text(json.dumps(data, indent=2))


def load(name: str) -> Optional[dict]:
    """Load state for a named sandbox, or None if not found."""
    p = _state_path(name)
    if not p.exists():
        return None
    try:
        return json.loads(p.read_text())
    except (json.JSONDecodeError, OSError):
        return None


def update(name: str, **fields: Any) -> None:
    """Update specific fields in an existing state file."""
    data = load(name)
    if data is None:
        return
    data.update(fields)
    _state_path(name).write_text(json.dumps(data, indent=2))


def delete(name: str) -> None:
    """Remove the state file for a sandbox."""
    _state_path(name).unlink(missing_ok=True)


def list_all() -> list[dict]:
    """Return all state file contents."""
    if not SANDBOX_STATE_DIR.exists():
        return []
    result = []
    for p in SANDBOX_STATE_DIR.glob("*.json"):
        try:
            result.append(json.loads(p.read_text()))
        except (json.JSONDecodeError, OSError):
            pass
    return result
