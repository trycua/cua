"""Anonymous usage telemetry.

What is sent, what never is, and how to turn it off:
https://cua.ai/docs/cua-sdk/concepts/telemetry. The same switches as the ``cua`` CLI
and the Spaces app: ``DO_NOT_TRACK=1``, ``CUA_TELEMETRY=0``, or
``cua.telemetry.disable()`` (writes ``[telemetry] enabled = "off"`` to
``$CUA_HOME/config.toml``)::

    import cua.telemetry as t
    print(t.status().enabled)
    t.disable()
    print(t.show_last(5))
"""

from __future__ import annotations

import json
from typing import Any, List

from . import _native

__all__ = [
    "status",
    "enable",
    "disable",
    "show_last",
    "schema",
    "reset_id",
]


def status() -> "_native.TelemetryStatus":
    """Whether usage telemetry is on, why, and the exact envelope every event carries."""
    return _native.telemetry_status()


def enable() -> "_native.TelemetryStatus":
    """Turn usage telemetry on for this machine (the environment still wins)."""
    return _native.telemetry_set_enabled(True)


def disable() -> "_native.TelemetryStatus":
    """Turn usage telemetry off for this machine."""
    return _native.telemetry_set_enabled(False)


def show_last(limit: int = 20) -> List[Any]:
    """The last events queued or sent from this machine, exactly as sent."""
    return json.loads(_native.telemetry_show_last(limit))


def schema() -> Any:
    """Every event and property that may be sent, with its allowed values."""
    return json.loads(_native.telemetry_schema_json())


def reset_id() -> None:
    """Delete the anonymous install id and its salt."""
    _native.telemetry_reset_id()

