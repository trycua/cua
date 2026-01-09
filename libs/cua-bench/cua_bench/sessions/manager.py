"""Session manager for creating and managing async container sessions."""

import json
import os
import time
from pathlib import Path
from typing import Any, Dict, List, Optional

from .providers.base import SessionProvider
from .providers.cloud import CloudProvider
from .providers.docker import DockerProvider


def _get_state_dir() -> Path:
    """Get XDG state directory for cua-bench."""
    xdg_state = os.environ.get("XDG_STATE_HOME", os.path.expanduser("~/.local/state"))
    return Path(xdg_state) / "cua-bench"


# Session storage path
RUNS_FILE = _get_state_dir() / "runs.json"


def make(provider_name: str, env_type: Optional[str] = None) -> SessionProvider:
    """Create a session provider for the specified provider.

    Args:
        provider_name: Name of the provider:
            - "local": Run locally using Docker (webtop) or QEMU/KVM (winarena)
            - "cloud": Run on CUA Cloud (GCP Batch for webtop, Azure Batch for winarena)
            - "docker": (legacy) Alias for "local"
        env_type: Optional environment type hint ("webtop" or "winarena").
            Used by local provider to select appropriate backend.

    Returns:
        SessionProvider instance

    Raises:
        ValueError: If provider is not supported
    """
    # Normalize provider name (support legacy aliases)
    normalized = provider_name.lower()
    if normalized in ("docker", "local"):
        # Local execution - uses Docker for webtop, can use winarena for Windows
        # The DockerProvider handles both via task.computer configuration
        return DockerProvider()
    elif normalized == "cloud":
        return CloudProvider()
    else:
        raise ValueError(
            f"Unknown provider: {provider_name}. "
            "Supported providers: 'local' (Docker/QEMU), 'cloud' (CUA Cloud API)"
        )


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
    """Save runs to the storage file."""
    RUNS_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(RUNS_FILE, "w") as f:
        json.dump(runs, f, indent=2)


def add_session(session_data: Dict[str, Any]) -> None:
    """Add a new session to the storage.

    Args:
        session_data: Session metadata dict
    """
    runs = _load_runs()
    session_id = session_data["session_id"]

    # Add timestamp if not present
    if "created_at" not in session_data:
        session_data["created_at"] = time.time()

    runs[session_id] = session_data
    _save_runs(runs)


def remove_session(session_id: str) -> None:
    """Remove a session from storage.

    Args:
        session_id: Session identifier
    """
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
