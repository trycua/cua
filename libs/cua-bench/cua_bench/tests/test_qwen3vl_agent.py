"""Qwen3VLAgent step budget and computer handler (hermetic: fake ComputerAgent and session)."""

from __future__ import annotations

import cua_agent
import pytest
from cua_agent.types import ToolError
from cua_bench.agents import FailureMode
from cua_bench.agents.qwen3vl_agent import Qwen3VLAgent

from .fakes import FakeSandbox


def _model_turn(i: int) -> dict:
    return {
        "output": [
            {"type": "message", "role": "assistant", "content": [{"text": f"turn {i}"}]},
            {"type": "computer_call", "action": {"type": "click", "x": i, "y": i}},
        ],
        "usage": {},
    }


class FakeComputerAgent:
    """Yields like ComputerAgent.run: a model turn, then (once resumed) the action's result."""

    executed: list = []

    def __init__(self, **kwargs) -> None:
        pass

    async def run(self, instruction):
        for i in range(1, 100):
            yield _model_turn(i)
            # ComputerAgent runs the action only when the consumer asks for the next item.
            FakeComputerAgent.executed.append(i)
            yield {"output": [{"type": "computer_call_output", "output": {}}], "usage": {}}


class FakeSession:
    def __init__(self, sandbox=None, error: Exception | None = None) -> None:
        self.sandbox = sandbox
        self.error = error
        self.executed: list = []

    async def execute_action(self, action) -> None:
        if self.error is not None:
            raise self.error
        self.executed.append(action)


@pytest.fixture
def fake_computer_agent(monkeypatch):
    FakeComputerAgent.executed = []
    monkeypatch.setattr(cua_agent, "ComputerAgent", FakeComputerAgent)
    return FakeComputerAgent


@pytest.mark.parametrize("max_steps", [1, 3, 15])
async def test_max_steps_runs_that_many_actions(fake_computer_agent, max_steps):
    agent = Qwen3VLAgent(model="openai/qwen3-vl", max_steps=max_steps)

    result = await agent.perform_task("task", FakeSession())

    assert fake_computer_agent.executed == list(range(1, max_steps + 1))
    assert result.failure_mode == FailureMode.MAX_STEPS_EXCEEDED


async def test_a_rejected_action_is_a_tool_error_for_the_model():
    session = FakeSession(error=RuntimeError('env: protocol error: unknown key "click"'))
    computer = Qwen3VLAgent(model="openai/qwen3-vl")._create_custom_computer(session)

    with pytest.raises(ToolError, match='unknown key "click"'):
        await computer["keypress"](["shift", "click"])


async def test_an_unknown_button_is_a_tool_error_for_the_model():
    computer = Qwen3VLAgent(model="openai/qwen3-vl")._create_custom_computer(FakeSession())

    with pytest.raises(ToolError, match="Unknown button type: back"):
        await computer["click"](1, 2, "back")


async def test_key_down_and_key_up_hold_keys_on_the_sandbox_keyboard():
    sandbox = FakeSandbox()
    computer = Qwen3VLAgent(model="openai/qwen3-vl")._create_custom_computer(
        FakeSession(sandbox=sandbox)
    )

    await computer["key_down"](["ctrl", "shift"])
    await computer["key_up"](["ctrl", "shift"])
    await computer["key_down"]("alt")

    assert [(name, args) for name, args, _ in sandbox.actions] == [
        ("key_down", ("ctrl",)),
        ("key_down", ("shift",)),
        ("key_up", ("shift",)),
        ("key_up", ("ctrl",)),
        ("key_down", ("alt",)),
    ]


async def test_key_down_without_a_sandbox_is_a_tool_error():
    computer = Qwen3VLAgent(model="openai/qwen3-vl")._create_custom_computer(FakeSession())

    with pytest.raises(ToolError, match="key_down/key_up are not supported by this session"):
        await computer["key_down"](["shift"])
