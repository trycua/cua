"""Property-based oracle result validation using the external Hypothesis library."""
import asyncio
import math
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from hypothesis import given, settings, strategies as st

from cua_bench import DoneAction, run_single_task


@settings(max_examples=120, deadline=None)
@given(st.one_of(
    st.floats(allow_nan=True, allow_infinity=True, width=64),
    st.integers(min_value=-10**12, max_value=10**12),
))
def test_generated_oracle_rewards_never_inflate_success(reward):
    observed = {"closed": False, "steps": 0}

    class ControlledEnvironment:
        evaluate_task_fn = object()
        solve_task_fn = None

        async def reset(self, task_id=0):
            return b"initial", SimpleNamespace(description="property test")

        async def step(self, action):
            observed["steps"] += 1
            return b"next"

        async def evaluate(self):
            return reward

        async def close(self):
            observed["closed"] = True

    with patch("cua_bench.runners.make", return_value=ControlledEnvironment()):
        result = asyncio.run(run_single_task(
            Path("property-task"), agent_fn=lambda screenshot, task: DoneAction(), max_steps=2
        ))

    assert observed["closed"] is True
    assert result.steps == observed["steps"] == 1
    valid = math.isfinite(reward) and 0 <= reward <= 1
    if valid:
        assert result.error is None
        assert result.reward == float(reward)
        assert result.success is (reward >= 0.5)
    else:
        assert result.success is False
        assert result.reward == 0.0
        assert result.error is not None


@settings(max_examples=64, deadline=None)
@given(
    st.booleans(),
    st.sampled_from([0.0, 0.25, 0.5, 0.9, 1.0]),
)
def test_cleanup_failure_cannot_produce_success(should_fail_close, reward):
    closed = {"attempted": False}

    class CleanupEnvironment:
        evaluate_task_fn = object()
        solve_task_fn = None

        async def reset(self, task_id=0):
            return b"initial", SimpleNamespace()

        async def step(self, action):
            return b"next"

        async def evaluate(self):
            return reward

        async def close(self):
            closed["attempted"] = True
            if should_fail_close:
                raise RuntimeError("injected teardown failure")

    with patch("cua_bench.runners.make", return_value=CleanupEnvironment()):
        result = asyncio.run(run_single_task(
            Path("cleanup-task"), agent_fn=lambda screenshot, task: DoneAction(), max_steps=2
        ))
    assert closed["attempted"]
    assert result.steps == 1
    if should_fail_close:
        assert result.success is False
        assert result.reward == 0.0
        assert "Environment cleanup failed" in result.error
    else:
        assert result.error is None
        assert result.reward == reward
        assert result.success is (reward >= 0.5)
