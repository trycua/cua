"""Evaluator-side helpers for the web tasks: the saved state and the event log."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from . import common


def load(workspace: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    state = common.read_json(workspace / "state" / "state.json", {}) or {}
    events = common.read_jsonl(workspace / "state" / "events.jsonl")
    return (state if isinstance(state, dict) else {}), events


def of_type(events: list[dict[str, Any]], kind: str) -> list[dict[str, Any]]:
    return [e for e in events if e.get("type") == kind]


def norm(value: Any) -> str:
    return " ".join(value.split()) if isinstance(value, str) else ""
