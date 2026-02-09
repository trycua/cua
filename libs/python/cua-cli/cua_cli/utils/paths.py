"""XDG Base Directory helpers for CUA CLI."""

import os
from pathlib import Path


def get_xdg_data_home() -> Path:
    """Get XDG_DATA_HOME, defaulting to ~/.local/share per spec."""
    xdg_data = os.environ.get("XDG_DATA_HOME")
    if xdg_data:
        return Path(xdg_data)
    return Path.home() / ".local" / "share"


def get_xdg_state_home() -> Path:
    """Get XDG_STATE_HOME, defaulting to ~/.local/state per spec."""
    xdg_state = os.environ.get("XDG_STATE_HOME")
    if xdg_state:
        return Path(xdg_state)
    return Path.home() / ".local" / "state"


def get_data_dir() -> Path:
    """Get the CUA data directory (for images)."""
    return get_xdg_data_home() / "cua"


def get_state_dir() -> Path:
    """Get the CUA state directory (for registries)."""
    return get_xdg_state_home() / "cua"
