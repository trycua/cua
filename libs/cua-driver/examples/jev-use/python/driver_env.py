"""Environment for the Cua Driver process started over MCP stdio.

The MCP SDK's stdio transport passes only a small default allowlist (HOME,
PATH, USER, ...) on POSIX. On Linux that drops the desktop session, so the
Driver cannot reach the display or session bus and the isolated browser exits
before exposing DevTools. Forward the SDK defaults plus the desktop-session and
Cua Driver variables only; provider credentials such as TYPESAFE_API_KEY stay
in the agent process.
"""

from __future__ import annotations

import os
from collections.abc import Mapping

from mcp.client.stdio import get_default_environment

DESKTOP_SESSION_VARS = (
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "AT_SPI_BUS_ADDRESS",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
)
DRIVER_VAR_PREFIX = "CUA_DRIVER_"


def driver_environment(source: Mapping[str, str] | None = None) -> dict[str, str]:
    """Return the SDK default environment plus desktop-session and Driver variables."""
    source = os.environ if source is None else source
    env = dict(get_default_environment())
    for key, value in source.items():
        if key in DESKTOP_SESSION_VARS or key.startswith(DRIVER_VAR_PREFIX):
            env[key] = value
    return env
