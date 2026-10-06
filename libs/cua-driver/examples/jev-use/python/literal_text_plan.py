"""Caller-owned literal text fast path; this does not interpret an open goal."""
from __future__ import annotations

import asyncio
import math
import time
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Protocol, Sequence

from core import Candidate
from native import NativeObservation, NativeObservationError
from native_roles import Platform
from sources import NativeAccessibilitySource


class DriverLike(Protocol):
    """Match run.Driver.call: raise on MCP errors and structured refusals."""
    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]: ...


class LiteralPlanHandoff(RuntimeError):
    """Stop before repeating any input whose effect is uncertain."""


@dataclass(frozen=True)
class LiteralTextStep:
    label: str
    value: str


async def execute_literal_text_plan(
    driver: DriverLike,
    *,
    pid: int,
    window_id: int,
    platform: Platform,
    steps: Sequence[LiteralTextStep],
    verify_step: Callable[[LiteralTextStep], Awaitable[bool]],
    verify_final: Callable[[], Awaitable[bool]],
    verify_context: Callable[[NativeObservation], bool],
    timeout_s: float = 30,
    choose: Callable[[Candidate, NativeObservation], Awaitable[str]] | None = None,
) -> dict[str, Any]:
    """Execute up to three explicit text setters with fresh binding and independent proof.

    The caller supplies literal values, exact labels, a same-record/context guard,
    and independent application oracles. ``choose`` is an optional comparison hook
    that may return only the sole complete candidate's ID; it cannot add authority.
    Driver errors/timeouts stop the plan. The caller must observe uncertain effects
    before starting another plan; this function never retries an input.
    """
    plan = tuple(steps)
    if not 1 <= len(plan) <= 3 or any(
        not isinstance(step, LiteralTextStep)
        or not isinstance(step.label, str) or not step.label
        or not isinstance(step.value, str)
        for step in plan
    ) or len({step.label for step in plan}) != len(plan):
        raise LiteralPlanHandoff("Expected one to three distinct literal text steps")
    if not isinstance(timeout_s, (int, float)) or not math.isfinite(timeout_s) or timeout_s <= 0:
        raise LiteralPlanHandoff("Invalid deadline")
    deadline = time.monotonic() + timeout_s

    async def bounded(awaitable: Awaitable[Any]) -> Any:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            if hasattr(awaitable, "close"):
                awaitable.close()  # do not leave an unawaited coroutine
            raise LiteralPlanHandoff("Plan deadline exceeded")
        return await asyncio.wait_for(awaitable, timeout=remaining)

    async def observe() -> NativeAccessibilitySource:
        for attempt in range(2):
            payload = await bounded(driver.call("get_window_state", {
                "pid": pid, "window_id": window_id,
                "include_accessibility_tree": True, "include_screenshot": True,
                "timeout_ms": 1000 if attempt == 0 else 5000,
            }))
            try:
                observation = NativeObservation.from_window_state(
                    payload, expected_pid=pid, expected_window_id=window_id,
                )
            except NativeObservationError as exc:
                raise LiteralPlanHandoff("Observation owner/schema mismatch") from exc
            source = NativeAccessibilitySource.from_observation(observation, platform)
            if observation.truncated or (observation.partial and not source.controls):
                if attempt == 0:
                    continue  # same bounded reobservation rule as run_native.py
                raise LiteralPlanHandoff("No complete actionable observation")
            if not verify_context(observation):
                raise LiteralPlanHandoff("Caller context guard failed")
            return source
        raise AssertionError("unreachable")

    receipts = []
    for step in plan:
        before = await observe()
        control = before.find("text_input", step.label)
        if control is None or control.handle.risk:
            raise LiteralPlanHandoff("Text selector is ambiguous, unavailable, or risky")
        candidate = before.type_text(control, step.value, candidate_id=control.handle.id,
                                     description=f"Set {step.label} to the caller-supplied literal")
        if choose is not None and await bounded(choose(candidate, before.observation)) != candidate.id:
            raise LiteralPlanHandoff("Chooser did not select the sole allowed action")
        fresh = await observe()
        current = fresh.find("text_input", step.label)
        if current is None or current.handle.risk:
            raise LiteralPlanHandoff("Text selector changed")
        # Stable ID includes actionable ancestry. Values and selected state must
        # remain unchanged; observation-local token/index are intentionally fresh.
        old, new = control.handle, current.handle
        if (old.id, old.role_class, old.label, old.value, old.selected) != (
            new.id, new.role_class, new.label, new.value, new.selected
        ):
            raise LiteralPlanHandoff("Text binding changed before input")
        bound = fresh.type_text(current, step.value, candidate_id=new.id,
                                description=candidate.description)
        await bounded(driver.call(bound.tool, dict(bound.arguments)))
        if not await bounded(verify_step(step)):
            raise LiteralPlanHandoff("Independent step verification failed")
        receipts.append({"label": step.label, "verified": True})
    if not await bounded(verify_final()):
        raise LiteralPlanHandoff("Independent final verification failed")
    return {"status": "verified_complete", "steps": receipts}
