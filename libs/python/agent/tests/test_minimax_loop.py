import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from cua_agent.decorators import find_agent_config
from cua_agent.loops.generic_vlm import GenericVlmConfig
from cua_agent.loops.minimax import MiniMaxComputerUseConfig


class FakeComputer:
    async def get_dimensions(self):
        return 1280, 720

    async def get_environment(self):
        return "linux"

    async def screenshot(self):
        return "c2NyZWVu"


class FakeResponse:
    def __init__(self, message):
        self.usage = MagicMock()
        self._hidden_params = {}
        self._message = message

    def model_dump(self):
        return {"choices": [{"message": self._message}]}


def test_minimax_m3_uses_image_capable_loop():
    for model in ("openai/MiniMax-M3", "anthropic/MiniMax-M3", "MiniMax-M3"):
        config = find_agent_config(model)
        assert config is not None
        assert config.agent_class is MiniMaxComputerUseConfig


def test_minimax_m27_remains_outside_image_capable_loop():
    config = find_agent_config("openai/MiniMax-M2.7")
    assert config is not None
    assert config.agent_class is GenericVlmConfig


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("model", "api_base"),
    [
        ("openai/MiniMax-M3", "https://api.minimax.io/v1"),
        ("openai/MiniMax-M3", "https://api.minimaxi.com/v1"),
        ("anthropic/MiniMax-M3", "https://api.minimax.io/anthropic"),
        ("anthropic/MiniMax-M3", "https://api.minimaxi.com/anthropic"),
    ],
)
async def test_predict_step_forwards_image_tools_and_endpoint(model, api_base):
    response = FakeResponse(
        {
            "content": "",
            "tool_calls": [
                {
                    "type": "function",
                    "id": "call-1",
                    "function": {
                        "name": "computer",
                        "arguments": json.dumps({"action": "click", "x": 12, "y": 34}),
                    },
                }
            ],
        }
    )
    computer = FakeComputer()

    with (
        patch(
            "cua_agent.loops.minimax.litellm.acompletion", new=AsyncMock(return_value=response)
        ) as completion,
        patch("cua_agent.loops.minimax._usage_from_response", return_value={}),
    ):
        result = await MiniMaxComputerUseConfig().predict_step(
            messages=[{"role": "user", "content": "Open the settings panel"}],
            model=model,
            tools=[{"type": "computer", "computer": computer}],
            computer_handler=computer,
            api_base=api_base,
            api_key="test-key",
        )

    request = completion.await_args.kwargs
    assert request["api_base"] == api_base
    assert request["messages"][-1]["content"][0] == {
        "type": "image_url",
        "image_url": {
            "url": "data:image/png;base64,c2NyZWVu",
            "detail": "default",
        },
    }
    assert request["tools"][0]["function"]["name"] == "computer"
    assert result["output"][-1]["type"] == "computer_call"
    assert result["output"][-1]["action"] == {"type": "click", "x": 12, "y": 34}


@pytest.mark.asyncio
async def test_predict_click_uses_image_and_returns_coordinates():
    response = FakeResponse(
        {
            "content": "",
            "tool_calls": [
                {
                    "type": "function",
                    "id": "call-1",
                    "function": {
                        "name": "computer",
                        "arguments": json.dumps({"action": "click", "x": 45, "y": 67}),
                    },
                }
            ],
        }
    )

    with patch(
        "cua_agent.loops.minimax.litellm.acompletion",
        new=AsyncMock(return_value=response),
    ) as completion:
        result = await MiniMaxComputerUseConfig().predict_click(
            model="openai/MiniMax-M3",
            image_b64="c2NyZWVu",
            instruction="Click the primary button",
            api_base="https://api.minimax.io/v1",
            api_key="test-key",
        )

    assert result == (45, 67)
    request = completion.await_args.kwargs
    assert request["messages"][0]["content"][1]["image_url"]["detail"] == "default"
    assert request["tools"][0]["function"]["parameters"]["required"] == [
        "action",
        "x",
        "y",
    ]
