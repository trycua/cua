"""Caller-owned literal text plans for native jev-use (#4749).

When the caller already holds the exact text-field labels and literal values,
asking a chooser to pick each sole permitted setter adds provider calls without
adding intent. ``execute_literal_text_plan`` runs one to three such setters
directly. It never interprets an open-ended goal: every label and value comes
from the caller, and anything it cannot bind exactly hands control back.

Each step observes the window, binds the label to exactly one eligible,
non-risky ``text_input`` (``NativeAccessibilitySource``), observes again, and
types only if the fresh binding has the same stable ID, label, value and
selection. The fresh observation's token is used. The caller's context guard
runs on every observation, and the caller's independent oracles confirm each
step and the final state. A Driver error, refusal or the plan deadline stops
the plan; no input is ever retried.
"""

from __future__ import annotations

import asyncio
import math
import time
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Protocol, Sequence

from core import Candidate
from native import NativeObservation, NativeObservationError
from native_roles import Platform
from run_native import REOBSERVE_TIMEOUT_MS, observe
from sources import NativeAccessibilitySource

MAX_STEPS = 3


class DriverLike(Protocol):
    """Match ``run.Driver.call``: raise on MCP errors and structured refusals."""

    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]: ...


class LiteralPlanHandoff(RuntimeError):
    """The plan stopped; the caller decides what happens next."""


@dataclass(frozen=True)
class LiteralTextStep:
    label: str
    value: str


def _checked_steps(steps: Sequence[LiteralTextStep]) -> tuple[LiteralTextStep, ...]:
    plan = tuple(steps)
    if (
        not 1 <= len(plan) <= MAX_STEPS
        or any(
            not isinstance(step, LiteralTextStep)
            or not isinstance(step.label, str)
            or not step.label
            or not isinstance(step.value, str)
            for step in plan
        )
        or len({step.label for step in plan}) != len(plan)
    ):
        raise LiteralPlanHandoff(f"Expected one to {MAX_STEPS} distinct literal text steps")
    return plan


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
    """Run the caller's literal text setters and return a verified receipt.

    ``verify_context`` must reject another record, modal or owner even when its
    labels match. ``verify_step`` and ``verify_final`` must read application
    state independently of the tree used to bind the action. ``choose`` is an
    optional comparison hook: it may only return the sole candidate's ID and
    cannot change the tool or its arguments.
    """
    plan = _checked_steps(steps)
    if isinstance(timeout_s, bool) or not isinstance(timeout_s, (int, float)):
        raise LiteralPlanHandoff("Invalid deadline")
    if not math.isfinite(timeout_s) or timeout_s <= 0:
        raise LiteralPlanHandoff("Invalid deadline")
    deadline = time.monotonic() + timeout_s

    async def bounded(awaitable: Awaitable[Any]) -> Any:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            if hasattr(awaitable, "close"):
                awaitable.close()
            raise LiteralPlanHandoff("Plan deadline exceeded")
        return await asyncio.wait_for(awaitable, timeout=remaining)

    async def bind() -> NativeAccessibilitySource:
        # Same rule as run_native.observe_step: a truncated tree, or a partial
        # one with no eligible control, is observed once more with a larger budget.
        for timeout_ms in (None, REOBSERVE_TIMEOUT_MS):
            try:
                observation = await bounded(observe(driver, None, pid, window_id, timeout_ms))
            except NativeObservationError as exc:
                raise LiteralPlanHandoff("Observation owner or schema mismatch") from exc
            if not verify_context(observation):
                raise LiteralPlanHandoff("Caller context guard failed")
            source = NativeAccessibilitySource.from_observation(observation, platform)
            if observation.truncated or (observation.partial and not source.controls):
                continue
            return source
        raise LiteralPlanHandoff("No complete actionable observation")

    receipts = []
    for step in plan:
        before = await bind()
        control = before.find("text_input", step.label)
        if control is None or control.handle.risk:
            raise LiteralPlanHandoff("Text field is ambiguous, unavailable, or risky")
        candidate = before.type_text(
            control,
            step.value,
            candidate_id=control.handle.id,
            description=f"Set {step.label} to the caller-supplied literal",
        )
        if choose is not None:
            if await bounded(choose(candidate, before.observation)) != candidate.id:
                raise LiteralPlanHandoff("Chooser did not select the sole allowed action")
        fresh = await bind()
        current = fresh.find("text_input", step.label)
        if current is None or current.handle.risk:
            raise LiteralPlanHandoff("Text field changed before input")
        old, new = control.handle, current.handle
        # The stable ID covers actionable ancestry; the token is observation-local
        # and is expected to change.
        if (old.id, old.role_class, old.label, old.value, old.selected) != (
            new.id,
            new.role_class,
            new.label,
            new.value,
            new.selected,
        ):
            raise LiteralPlanHandoff("Text binding changed before input")
        bound = fresh.type_text(
            current, step.value, candidate_id=new.id, description=candidate.description
        )
        await bounded(driver.call(bound.tool, dict(bound.arguments)))
        if not await bounded(verify_step(step)):
            raise LiteralPlanHandoff("Independent step verification failed")
        receipts.append({"label": step.label, "verified": True})
    if not await bounded(verify_final()):
        raise LiteralPlanHandoff("Independent final verification failed")
    return {"status": "verified_complete", "steps": receipts}


def validate_literal_text_plan(
    payload: Any,
    *,
    pid: int,
    window_id: int,
    expected: Sequence[LiteralTextStep],
) -> tuple[LiteralTextStep, ...]:
    """Accept a model plan only if it transcribes the caller's steps exactly.

    The payload must be ``{"owner": {"pid", "window_id"}, "steps": [{"label",
    "value"}, ...]}`` with the caller's owner and the caller's steps in order.
    Execution still binds every label freshly and runs the caller's checks.
    """
    supplied = _checked_steps(expected)
    if not isinstance(payload, dict) or set(payload) != {"owner", "steps"}:
        raise LiteralPlanHandoff("Unexpected plan fields")
    owner = payload["owner"]
    if (
        not isinstance(owner, dict)
        or set(owner) != {"pid", "window_id"}
        or any(type(owner[key]) is not int for key in owner)
        or owner != {"pid": pid, "window_id": window_id}
    ):
        raise LiteralPlanHandoff("Plan owner differs from caller")
    entries = payload["steps"]
    if not isinstance(entries, list) or len(entries) != len(supplied):
        raise LiteralPlanHandoff("Plan must preserve every caller step")
    for entry, step in zip(entries, supplied):
        if not isinstance(entry, dict) or set(entry) != {"label", "value"}:
            raise LiteralPlanHandoff("Unexpected step fields")
        if entry != {"label": step.label, "value": step.value}:
            raise LiteralPlanHandoff("Plan changes authorized literals or their order")
    return supplied
