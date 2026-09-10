"""Runtime adapter contracts."""

from __future__ import annotations

from collections.abc import Mapping
from typing import Any, Protocol

from cua_bench_runtime.model import (
    AgentOutcome,
    CleanupReport,
    EnvironmentHandle,
    Evaluation,
    ObserverReport,
    TrialContext,
)
from cua_bench_runtime.signals import InterruptFlag


class EnvironmentAdapter(Protocol):
    name: str

    def setup(self, context: TrialContext) -> EnvironmentHandle: ...

    def export(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome,
    ) -> EnvironmentHandle: ...

    def cleanup(
        self,
        context: TrialContext,
        handle: EnvironmentHandle | None,
        timeout_seconds: float,
    ) -> CleanupReport: ...


class HarnessAdapter(Protocol):
    name: str

    def run(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> AgentOutcome: ...


class DriverAdapter(Protocol):
    name: str

    def resolve(self, context: TrialContext) -> Mapping[str, Any]: ...


class ObserverAdapter(Protocol):
    name: str

    def start(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        requirements: tuple[Mapping[str, Any], ...],
    ) -> None: ...

    def finish(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome | None,
    ) -> ObserverReport: ...


class EvaluatorAdapter(Protocol):
    name: str

    def evaluate(
        self,
        context: TrialContext,
        handle: EnvironmentHandle,
        outcome: AgentOutcome,
        interrupt: InterruptFlag,
        timeout_seconds: float,
    ) -> Evaluation: ...
