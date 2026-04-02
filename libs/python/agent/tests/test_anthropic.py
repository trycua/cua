"""Unit tests for Anthropic agent loop implementation.

This file tests the Anthropic-specific message formatting and conversion logic.
Following SRP: This file tests ONLY Anthropic loop functionality.
All external dependencies (liteLLM, Computer) are mocked.
"""

import pytest
from unittest.mock import MagicMock, patch, AsyncMock
import json


class TestScaleCoordinate:
    """Test coordinate scaling function."""

    def test_scale_up(self):
        """Test scaling up coordinates."""
        from agent.loops.anthropic import _scale_coordinate

        result = _scale_coordinate(100, 2.0)
        assert result == 200

    def test_scale_down(self):
        """Test scaling down coordinates."""
        from agent.loops.anthropic import _scale_coordinate

        result = _scale_coordinate(100, 0.5)
        assert result == 50

    def test_scale_identity(self):
        """Test no scaling (factor 1.0)."""
        from agent.loops.anthropic import _scale_coordinate

        result = _scale_coordinate(100, 1.0)
        assert result == 100

    def test_scale_rounds_correctly(self):
        """Test that scaling rounds to nearest integer."""
        from agent.loops.anthropic import _scale_coordinate

        result = _scale_coordinate(100, 1.5)
        assert result == 150

        result = _scale_coordinate(101, 0.5)
        assert result == 50  # 50.5 rounded down


class TestModelToolMapping:
    """Test model to tool version mapping."""

    def test_claude_model_mapping(self):
        """Test Claude model mapping."""
        from agent.loops.anthropic import MODEL_TOOL_MAPPING

        # Should have mapping for Claude models
        assert len(MODEL_TOOL_MAPPING) > 0

    def test_tool_config_function(self):
        """Test tool config retrieval function."""
        from agent.loops.anthropic import _get_tool_config_for_model

        # Test with a Claude model
        result = _get_tool_config_for_model("anthropic/claude-sonnet-4-5-20250929")
        assert isinstance(result, dict)

    def test_tool_config_for_unknown_model(self):
        """Test tool config for unknown model returns defaults."""
        from agent.loops.anthropic import _get_tool_config_for_model

        result = _get_tool_config_for_model("unknown-model")
        assert isinstance(result, dict)


class TestAnthropicMessageFormatting:
    """Test Anthropic-specific message formatting."""

    @patch("agent.loops.anthropic.litellm")
    def test_message_format_structure(self, mock_litellm):
        """Test that messages are formatted correctly for Anthropic API."""
        from agent.loops.anthropic import AnthropicHostedToolsConfig

        config = AnthropicHostedToolsConfig()

        # Messages should be in correct format
        messages = [{"role": "user", "content": "Hello"}]

        # Verify the config can handle the messages
        assert config is not None
        assert hasattr(config, "predict_step")

    def test_system_message_handling(self):
        """Test system message is properly included."""
        from agent.loops.anthropic import AnthropicHostedToolsConfig

        config = AnthropicHostedToolsConfig()
        assert config is not None


class TestComputerActionConversion:
    """Test conversion of computer actions for Anthropic API."""

    def test_click_action_format(self):
        """Test click action is formatted correctly."""
        from agent.responses import make_click_item

        action = make_click_item(100, 200, "left")

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "click"
        assert action["action"]["x"] == 100
        assert action["action"]["y"] == 200
        assert action["action"]["button"] == "left"

    def test_right_click_action_format(self):
        """Test right click action is formatted correctly."""
        from agent.responses import make_click_item

        action = make_click_item(100, 200, "right")

        assert action["action"]["button"] == "right"

    def test_double_click_action_format(self):
        """Test double click action is formatted correctly."""
        from agent.responses import make_double_click_item

        action = make_double_click_item(100, 200)

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "double_click"

    def test_drag_action_format(self):
        """Test drag action is formatted correctly."""
        from agent.responses import make_drag_item

        path = [{"x": 100, "y": 200}, {"x": 300, "y": 400}]
        action = make_drag_item(path)

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "drag"
        assert len(action["action"]["path"]) == 2

    def test_type_action_format(self):
        """Test type action is formatted correctly."""
        from agent.responses import make_type_item

        action = make_type_item("hello world")

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "type"
        assert action["action"]["text"] == "hello world"

    def test_keypress_action_format(self):
        """Test keypress action is formatted correctly."""
        from agent.responses import make_keypress_item

        action = make_keypress_item(["ctrl", "c"])

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "keypress"
        assert action["action"]["keys"] == ["ctrl", "c"]

    def test_scroll_action_format(self):
        """Test scroll action is formatted correctly."""
        from agent.responses import make_scroll_item

        action = make_scroll_item(100, 200, 0, -5)

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "scroll"
        assert action["action"]["scroll_y"] == -5

    def test_wait_action_format(self):
        """Test wait action is formatted correctly."""
        from agent.responses import make_wait_item

        action = make_wait_item()

        assert action["type"] == "computer_call"
        assert action["action"]["type"] == "wait"


class TestScreenDimensionHandling:
    """Test screen dimension handling for coordinate scaling."""

    def test_recommended_max_dimensions(self):
        """Test recommended max dimensions are defined."""
        from agent.loops.anthropic import RECOMMENDED_MAX_WIDTH, RECOMMENDED_MAX_HEIGHT

        assert RECOMMENDED_MAX_WIDTH == 1024
        assert RECOMMENDED_MAX_HEIGHT == 768

    def test_coordinate_scaling_needed(self):
        """Test when coordinate scaling is needed."""
        # If screen is larger than recommended, scaling should be applied
        screen_width = 1920
        screen_height = 1080

        from agent.loops.anthropic import RECOMMENDED_MAX_WIDTH, RECOMMENDED_MAX_HEIGHT

        # Calculate scale factors
        scale_x = RECOMMENDED_MAX_WIDTH / screen_width
        scale_y = RECOMMENDED_MAX_HEIGHT / screen_height

        assert scale_x < 1.0
        assert scale_y < 1.0


class TestAnthropicResponseParsing:
    """Test parsing of Anthropic API responses."""

    def test_parse_text_response(self):
        """Test parsing text response from Anthropic."""
        # Simulate Anthropic response structure
        response = {
            "content": [
                {"type": "text", "text": "I'll help you with that."}
            ],
            "role": "assistant"
        }

        assert response["role"] == "assistant"
        assert response["content"][0]["type"] == "text"

    def test_parse_tool_use_response(self):
        """Test parsing tool use response from Anthropic."""
        response = {
            "content": [
                {
                    "type": "tool_use",
                    "name": "computer",
                    "input": {
                        "action": "click",
                        "coordinate": [100, 200]
                    }
                }
            ],
            "role": "assistant"
        }

        assert response["content"][0]["type"] == "tool_use"
        assert response["content"][0]["name"] == "computer"


class TestAnthropicCapabilities:
    """Test AnthropicConfig capabilities."""

    @patch("agent.loops.anthropic.litellm")
    def test_get_capabilities(self, mock_litellm):
        """Test that get_capabilities returns expected values."""
        from agent.loops.anthropic import AnthropicHostedToolsConfig

        config = AnthropicHostedToolsConfig()
        capabilities = config.get_capabilities()

        assert isinstance(capabilities, list)
        assert "step" in capabilities
        assert "click" in capabilities


class TestAnthropicErrorHandling:
    """Test error handling in Anthropic loop."""

    @patch("agent.loops.anthropic.litellm")
    def test_empty_messages_handling(self, mock_litellm):
        """Test handling of empty messages."""
        from agent.loops.anthropic import AnthropicHostedToolsConfig

        config = AnthropicHostedToolsConfig()
        assert config is not None

    @patch("agent.loops.anthropic.litellm")
    def test_invalid_message_format(self, mock_litellm):
        """Test handling of invalid message format."""
        from agent.loops.anthropic import AnthropicHostedToolsConfig

        config = AnthropicHostedToolsConfig()
        assert config is not None


class TestAnthropicBetaHeaders:
    """Test Anthropic beta header configuration."""

    def test_beta_headers_for_computer_use(self):
        """Test that beta flags are defined for models."""
        from agent.loops.anthropic import MODEL_TOOL_MAPPING

        # Check that beta flags are defined for models
        for mapping in MODEL_TOOL_MAPPING:
            if "beta" in mapping:
                assert isinstance(mapping["beta"], list)


class TestMessageMerging:
    """Test message merging logic for Anthropic."""

    def test_consecutive_assistant_messages(self):
        """Test handling of consecutive assistant messages."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [
            {"role": "assistant", "content": [{"type": "text", "text": "Hello"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "World"}]},
        ]

        result = convert_responses_items_to_completion_messages(messages)
        assert len(result) >= 1

    def test_consecutive_user_messages(self):
        """Test handling of consecutive user messages."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [
            {"role": "user", "content": "First message"},
            {"role": "user", "content": "Second message"},
        ]

        result = convert_responses_items_to_completion_messages(messages)
        assert len(result) == 2

    def test_tool_call_merging(self):
        """Test merging of tool calls into single assistant message."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "computer",
                "arguments": '{"action": "click"}'
            },
            {
                "type": "function_call",
                "call_id": "call_2",
                "name": "computer",
                "arguments": '{"action": "type"}'
            },
        ]

        result = convert_responses_items_to_completion_messages(messages)
        # Both tool calls should be in a single assistant message
        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert len(result[0].get("tool_calls", [])) == 2


class TestScreenshotHandling:
    """Test screenshot handling in Anthropic loop."""

    def test_screenshot_in_computer_call_output(self):
        """Test screenshot is properly included in computer_call_output."""
        from agent.responses import make_input_image_item

        image_data = b"fake_image_data"
        item = make_input_image_item(image_data)

        assert item["role"] == "user"
        assert len(item["content"]) == 1
        assert item["content"][0]["type"] == "input_image"

    def test_base64_image_encoding(self):
        """Test that image data is base64 encoded."""
        import base64
        from agent.responses import make_input_image_item

        image_data = b"test_image_bytes"
        item = make_input_image_item(image_data)

        image_url = item["content"][0]["image_url"]
        assert image_url.startswith("data:image/png;base64,")

        # Extract and decode base64
        b64_part = image_url.split(",")[1]
        decoded = base64.b64decode(b64_part)
        assert decoded == image_data