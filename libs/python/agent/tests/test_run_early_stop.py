"""ComputerAgent.run() when a callback stops the run before the first step."""

from unittest.mock import AsyncMock, patch

import pytest


class _RecordRunEnd:
    def __init__(self):
        self.run_end_calls = []

    async def on_run_end(self, kwargs, old_items, new_items):
        self.run_end_calls.append((kwargs, old_items, new_items))


@pytest.mark.asyncio
@patch("cua_agent.agent.litellm")
async def test_run_ends_cleanly_when_budget_is_already_spent(mock_litellm, disable_telemetry):
    from cua_agent import ComputerAgent
    from cua_agent.callbacks import BudgetManagerCallback

    mock_litellm.acompletion = AsyncMock()
    recorder = _RecordRunEnd()
    agent = ComputerAgent(
        model="anthropic/claude-sonnet-4-5-20250929",
        callbacks=[recorder],
        max_trajectory_budget={"max_budget": 1.0, "reset_after_each_run": False},
    )
    budget = next(cb for cb in agent.callbacks if isinstance(cb, BudgetManagerCallback))
    budget.total_cost = 1.5  # spent by an earlier run

    outputs = [chunk async for chunk in agent.run("open the browser")]

    assert outputs == []
    mock_litellm.acompletion.assert_not_called()
    assert len(recorder.run_end_calls) == 1
    kwargs, old_items, new_items = recorder.run_end_calls[0]
    assert kwargs["model"] == "anthropic/claude-sonnet-4-5-20250929"
    assert new_items == []
