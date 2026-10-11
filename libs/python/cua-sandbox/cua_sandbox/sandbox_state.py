"""Persistent state tracking for local sandboxes and Fleet claims.

Each running local sandbox or named Fleet claim writes a JSON file at
``$CUA_HOME/sandboxes/{name}.json`` (default ``~/.cua/sandboxes``). This lets later connect and delete commands route
to the original runtime or pre-created Fleet pool across process restarts.

Ephemeral sandboxes never write state files.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from cua_sandbox._paths import cua_home, guard_write, patched_or

#: Import-time default; tests may patch it. Use :func:`state_dir`.
SANDBOX_STATE_DIR = cua_home() / "sandboxes"
_IMPORT_DEFAULT = SANDBOX_STATE_DIR


def state_dir() -> Path:
    """``$CUA_HOME/sandboxes`` now (or the patched ``SANDBOX_STATE_DIR``)."""
    return patched_or(SANDBOX_STATE_DIR, _IMPORT_DEFAULT, "sandboxes")


def _state_path(name: str) -> Path:
    return state_dir() / f"{name}.json"


def save(
    name: str,
    *,
    runtime_type: str,
    image: dict,
    host: str,
    api_port: int,
    exposed_ports: Optional[dict] = None,
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
    network: Optional[str] = None,
    status: str = "running",
) -> None:
    """Write or overwrite the state file for a local sandbox."""
    guard_write(_state_path(name))
    state_dir().mkdir(parents=True, exist_ok=True)
    data: dict[str, Any] = {
        "name": name,
        "runtime_type": runtime_type,
        "image": image,
        "host": host,
        "api_port": api_port,
        "exposed_ports": exposed_ports,
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
        "network": network,
        "status": status,
        "created_at": datetime.now(timezone.utc).isoformat(),
    }
    guard_write(_state_path(name))
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
    guard_write(_state_path(name))
    _state_path(name).write_text(json.dumps(data, indent=2))


def delete(name: str) -> None:
    """Remove the state file for a sandbox."""
    _state_path(name).unlink(missing_ok=True)


def list_all() -> list[dict]:
    """Return all state file contents."""
    if not state_dir().exists():
        return []
    result = []
    for p in state_dir().glob("*.json"):
        try:
            result.append(json.loads(p.read_text()))
        except (json.JSONDecodeError, OSError):
            pass
    return result


def save_fleet_claim(name: str, pool_name: str, **extra: Any) -> None:
    """Persist the pool association for a named Fleet claim.

    Claims on managed pools also record ``managed=True``, the pool's
    ``spec_hash`` and the ``claim_ttl`` they were created with.
    """
    guard_write(_state_path(name))
    state_dir().mkdir(parents=True, exist_ok=True)
    data: dict[str, Any] = {
        "name": name,
        "runtime_type": "fleet",
        "pool_name": pool_name,
        "status": "running",
        "created_at": datetime.now(timezone.utc).isoformat(),
        **extra,
    }
    guard_write(_state_path(name))
    _state_path(name).write_text(json.dumps(data, indent=2))
