"""An opt-in caller experiment; Driver's action observation horizon is unchanged."""
from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable
from typing import TypeVar

Action = TypeVar("Action")
Observation = TypeVar("Observation")


async def action_with_read_warming(
    dispatch: Callable[[], Awaitable[Action]],
    read: Callable[[], Awaitable[Observation]],
    matches: Callable[[Observation], bool],
) -> tuple[Action, Observation]:
    """Warm a separate read-only session while one exact action is in flight.

    ``dispatch`` must issue exactly one already-bound action. ``read`` must be
    read-only, use a separate session, and target the same explicit PID/window.
    No advisory observation is returned or allowed to authorize another input.
    The caller still applies its ordinary verification policy to the returned
    action result and the mandatory observation taken after that action finishes.
    """
    action = asyncio.ensure_future(dispatch())
    try:
        for sample in range(2):
            advisory = await read()
            if matches(advisory):
                break
            if sample == 0:
                await asyncio.sleep(0.01)
        result = await asyncio.shield(action)
    except BaseException:
        # A read failure or ordinary caller cancellation must not cancel or
        # re-dispatch a mutation whose delivery may already have happened.
        try:
            await asyncio.shield(action)
        except BaseException:
            pass
        raise
    final = await read()
    return result, final
