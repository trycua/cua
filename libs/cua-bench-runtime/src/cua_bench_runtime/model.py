"""Frozen values exchanged across runtime adapters."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class TrialContext:
    trial_id: str
    task: Mapping[str, Any]
    task_path: Path
    config: Mapping[str, Any]
    trial_dir: Path
    artifacts: Path
    harness_workspace: Path
    emit: Callable[[str, dict[str, Any]], None]
    policy: Any | None = None


@dataclass(frozen=True)
class EnvironmentHandle:
    kind: str
    root: Path
    facts: Mapping[str, Any]


@dataclass(frozen=True)
class AgentOutcome:
    completed: bool
    exit_code: int | None
    duration_ms: int
    artifacts: tuple[str, ...]
    terminal_failure: str | None = None


@dataclass(frozen=True)
class ObserverReport:
    """Normalized events returned by a benchmark-owned driver observer.

    ``trust`` describes the isolation of the observer, not whether the task
    passed. Only ``certifying`` reports can make a GUI-required trial
    certifying; local desktop smokes may still prove participation with a
    ``non_certifying`` report.
    """

    name: str
    trust: str
    events: tuple[Mapping[str, Any], ...] = ()
    detail: str | None = None


@dataclass(frozen=True)
class Evaluation:
    passed: bool
    score: float | None
    detail: Mapping[str, Any]


@dataclass(frozen=True)
class CleanupReport:
    ok: bool
    reclaimed: tuple[str, ...] = ()
    error: str | None = None
