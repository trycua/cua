"""Workload-token authentication helpers."""

import os


def get_fleets_token() -> str | None:
    """Return the configured Fleets workload token, if present."""
    value = os.environ.get("FLEETS_TOKEN", "").strip()
    return value or None
