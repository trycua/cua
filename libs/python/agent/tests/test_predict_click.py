"""Tests for predict_click handling of zero coordinates.

Regression test for https://github.com/trycua/cua/issues/1400
"""

from unittest.mock import AsyncMock, patch

import pytest

MINIMAL_PNG_B64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ"
    "AAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
)


def _make_click_response(x, y):
    """Build a liteLLM completion response containing a click tool_use."""
    from unittest.mock import MagicMock

    message = MagicMock()
    message.content = [
        {
            "type": "tool_use",
            "id": "toolu_test",
            "name": "computer",
            "input": {"action": "click", "coordinate": [x, y]},
        }
    ]
    choice = MagicMock()
    choice.message = message
    response = MagicMock()
    response.choices = [choice]
    return response


def _make_text_only_response():
    from unittest.mock import MagicMock

    message = MagicMock()
    message.content = "No click here"
    choice = MagicMock()
    choice.message = message
    response = MagicMock()
    response.choices = [choice]
    return response


class TestAnthropicPredictClickZeroCoordinates:
    """Regression tests for #1400: predict_click must handle x=0 and y=0."""

    @pytest.mark.asyncio
    @patch("litellm.acompletion", new_callable=AsyncMock)
    async def test_predict_click_x_zero(self, mock_acompletion, disable_telemetry):
        from cua_agent.loops.anthropic import AnthropicHostedToolsConfig

        mock_acompletion.return_value = _make_click_response(0, 300)

        config = AnthropicHostedToolsConfig()
        result = await config.predict_click(
            model="anthropic/claude-sonnet-4-5-20250929",
            image_b64=MINIMAL_PNG_B64,
            instruction="click the left edge",
        )

        assert result is not None, "predict_click returned None for x=0"
        assert result == (0, 300)

    @pytest.mark.asyncio
    @patch("litellm.acompletion", new_callable=AsyncMock)
    async def test_predict_click_y_zero(self, mock_acompletion, disable_telemetry):
        from cua_agent.loops.anthropic import AnthropicHostedToolsConfig

        mock_acompletion.return_value = _make_click_response(500, 0)

        config = AnthropicHostedToolsConfig()
        result = await config.predict_click(
            model="anthropic/claude-sonnet-4-5-20250929",
            image_b64=MINIMAL_PNG_B64,
            instruction="click the top edge",
        )

        assert result is not None, "predict_click returned None for y=0"
        assert result == (500, 0)

    @pytest.mark.asyncio
    @patch("litellm.acompletion", new_callable=AsyncMock)
    async def test_predict_click_both_zero(self, mock_acompletion, disable_telemetry):
        from cua_agent.loops.anthropic import AnthropicHostedToolsConfig

        mock_acompletion.return_value = _make_click_response(0, 0)

        config = AnthropicHostedToolsConfig()
        result = await config.predict_click(
            model="anthropic/claude-sonnet-4-5-20250929",
            image_b64=MINIMAL_PNG_B64,
            instruction="click the top-left pixel",
        )

        assert result is not None, "predict_click returned None for (0, 0)"
        assert result == (0, 0)

    @pytest.mark.asyncio
    @patch("litellm.acompletion", new_callable=AsyncMock)
    async def test_predict_click_normal_coordinates(self, mock_acompletion, disable_telemetry):
        from cua_agent.loops.anthropic import AnthropicHostedToolsConfig

        mock_acompletion.return_value = _make_click_response(512, 384)

        config = AnthropicHostedToolsConfig()
        result = await config.predict_click(
            model="anthropic/claude-sonnet-4-5-20250929",
            image_b64=MINIMAL_PNG_B64,
            instruction="click the center",
        )

        assert result == (512, 384)

    @pytest.mark.asyncio
    @patch("litellm.acompletion", new_callable=AsyncMock)
    async def test_predict_click_no_click_action(self, mock_acompletion, disable_telemetry):
        from cua_agent.loops.anthropic import AnthropicHostedToolsConfig

        mock_acompletion.return_value = _make_text_only_response()

        config = AnthropicHostedToolsConfig()
        result = await config.predict_click(
            model="anthropic/claude-sonnet-4-5-20250929",
            image_b64=MINIMAL_PNG_B64,
            instruction="click something",
        )

        assert result is None
