"""predict_click keeps a click on the screen's top or left edge (issue #1400).

The model's reply is stubbed at the liteLLM boundary; each test reads only
what predict_click returns.
"""

from types import SimpleNamespace

import pytest
from cua_agent.loops import anthropic
from cua_agent.loops.anthropic import AnthropicHostedToolsConfig


def _reply(content):
    message = SimpleNamespace(content=content, tool_calls=None)
    return SimpleNamespace(choices=[SimpleNamespace(message=message)])


def _click_at(x, y):
    return _reply(
        [
            {
                "type": "tool_use",
                "id": "call_1",
                "name": "computer",
                "input": {"action": "left_click", "coordinate": [x, y]},
            }
        ]
    )


async def _predict(monkeypatch, reply):
    async def acompletion(**_):
        return reply

    monkeypatch.setattr(anthropic.litellm, "acompletion", acompletion)
    return await AnthropicHostedToolsConfig().predict_click(
        model="anthropic/claude-sonnet-5", image_b64="", instruction="the menu"
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("x, y", [(0, 100), (200, 0), (0, 0), (512, 384)])
async def test_a_click_anywhere_on_screen_is_returned(monkeypatch, x, y):
    assert await _predict(monkeypatch, _click_at(x, y)) == (x, y)


@pytest.mark.asyncio
async def test_a_reply_without_a_click_predicts_nothing(monkeypatch):
    assert await _predict(monkeypatch, _reply("no click")) is None
