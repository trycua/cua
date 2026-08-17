"""Network exposure checks for the computer-control server."""

from __future__ import annotations

import ipaddress
import os


class InsecureBindError(ValueError):
    """Raised when unauthenticated local mode would bind a remote interface."""


def _env_truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in ("1", "true", "yes", "y", "on")


def is_loopback_host(host: str) -> bool:
    """Return whether a bind or origin host is unambiguously loopback-only."""
    normalized = host.strip().lower().strip("[]")
    if normalized == "localhost":
        return True
    try:
        return ipaddress.ip_address(normalized).is_loopback
    except ValueError:
        return False


def resolve_bind_host(host: str) -> str:
    """Refuse unauthenticated non-loopback binds unless explicitly enabled."""
    if os.environ.get("CONTAINER_NAME") or is_loopback_host(host):
        return host
    if _env_truthy("CUA_ALLOW_UNAUTHENTICATED_REMOTE"):
        return host

    raise InsecureBindError(
        f"Refusing to bind {host!r} while local authentication is disabled. "
        "Use a loopback host, configure CONTAINER_NAME authentication, or set "
        "CUA_ALLOW_UNAUTHENTICATED_REMOTE=1 to acknowledge the remote shell and "
        "filesystem exposure."
    )
