"""CustomComputerHandler forwards key_down/key_up, or reports them as unknown actions."""

import pytest
from cua_agent.computers.custom import CustomComputerHandler
from cua_agent.types import ToolError


def _handler(**functions):
    return CustomComputerHandler({"screenshot": lambda: b"", **functions})


async def test_key_down_and_key_up_reach_the_functions():
    calls = []
    handler = _handler(
        key_down=lambda keys: calls.append(("down", keys)),
        key_up=lambda keys: calls.append(("up", keys)),
    )

    await handler.key_down(["shift"])
    await handler.key_up("shift")

    assert calls == [("down", ["shift"]), ("up", "shift")]


@pytest.mark.parametrize("action", ["key_down", "key_up"])
async def test_a_missing_function_is_an_unknown_action(action):
    with pytest.raises(ToolError, match=f"Unknown computer action: {action}"):
        await getattr(_handler(), action)(["shift"])
