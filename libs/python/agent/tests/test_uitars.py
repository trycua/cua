"""Unit tests for UITARS agent loop implementation.

This file tests the UITARS-specific message formatting, parsing, and conversion logic.
Following SRP: This file tests ONLY UITARS loop functionality.
All external dependencies (liteLLM, Computer) are mocked.
"""

import pytest
from unittest.mock import MagicMock, patch

from agent.loops.uitars import (
    parse_action,
    parse_uitars_response,
    convert_to_computer_actions,
    convert_uitars_messages_to_litellm,
    smart_resize,
    round_by_factor,
    ceil_by_factor,
    floor_by_factor,
    escape_single_quotes,
)


class TestSmartResize:
    """Test smart_resize function for image dimension handling."""

    def test_small_image_no_resize(self):
        """Images within min/max pixels should maintain divisibility by factor."""
        # 100x100 = 10000 pixels, within MIN_PIXELS and MAX_PIXELS
        height, width = smart_resize(100, 100)
        # Dimensions should be divisible by IMAGE_FACTOR (28)
        assert height % 28 == 0
        assert width % 28 == 0

    def test_large_image_downscale(self):
        """Large images should be downscaled to MAX_PIXELS."""
        # 5000x5000 = 25000000 pixels, much larger than MAX_PIXELS
        height, width = smart_resize(5000, 5000)
        assert height * width <= 16384 * 28 * 28  # MAX_PIXELS

    def test_small_image_upscale(self):
        """Very small images should be upscaled to MIN_PIXELS."""
        # 10x10 = 100 pixels, smaller than MIN_PIXELS
        height, width = smart_resize(10, 10)
        assert height * width >= 100 * 28 * 28  # MIN_PIXELS

    def test_divisibility_by_factor(self):
        """Output dimensions should be divisible by IMAGE_FACTOR."""
        height, width = smart_resize(123, 456)
        assert height % 28 == 0
        assert width % 28 == 0

    def test_aspect_ratio_preservation(self):
        """Aspect ratio should be preserved as closely as possible."""
        original_ratio = 1920 / 1080
        height, width = smart_resize(1080, 1920)
        new_ratio = width / height
        # Allow 10% tolerance due to rounding
        assert abs(new_ratio - original_ratio) / original_ratio < 0.1

    def test_extreme_aspect_ratio_raises_error(self):
        """Extremely wide/tall images should raise ValueError."""
        # 1000x1 has aspect ratio 1000, exceeding MAX_RATIO of 200
        with pytest.raises(ValueError, match="aspect ratio"):
            smart_resize(1, 1000)


class TestRoundByFactor:
    """Test rounding functions for dimension calculation."""

    def test_round_by_factor_exact(self):
        """Numbers already divisible by factor should remain unchanged."""
        assert round_by_factor(56, 28) == 56
        assert round_by_factor(112, 28) == 112

    def test_round_by_factor_round_up(self):
        """Numbers should round to nearest multiple."""
        # 50 is closer to 56 than 28 (distance: 6 vs 22)
        assert round_by_factor(50, 28) == 56
        # 40 is closer to 28 than 56 (distance: 12 vs 16)
        assert round_by_factor(40, 28) == 28

    def test_round_by_factor_round_down(self):
        """Numbers should round to nearest multiple."""
        assert round_by_factor(30, 28) == 28  # 30 is closer to 28 than 56


class TestCeilByFactor:
    """Test ceiling by factor function."""

    def test_ceil_exact(self):
        """Numbers already divisible should remain unchanged."""
        assert ceil_by_factor(56, 28) == 56

    def test_ceil_up(self):
        """Numbers should round up to next multiple."""
        assert ceil_by_factor(30, 28) == 56
        assert ceil_by_factor(29, 28) == 56
        assert ceil_by_factor(1, 28) == 28


class TestFloorByFactor:
    """Test floor by factor function."""

    def test_floor_exact(self):
        """Numbers already divisible should remain unchanged."""
        assert floor_by_factor(56, 28) == 56

    def test_floor_down(self):
        """Numbers should round down to previous multiple."""
        assert floor_by_factor(55, 28) == 28
        assert floor_by_factor(83, 28) == 56


class TestEscapeSingleQuotes:
    """Test single quote escaping for action strings."""

    def test_no_quotes(self):
        """Strings without quotes should remain unchanged."""
        assert escape_single_quotes("hello world") == "hello world"

    def test_single_quotes(self):
        """Single quotes should be escaped."""
        assert escape_single_quotes("it's") == "it\\'s"
        assert escape_single_quotes("don't do it") == "don\\'t do it"

    def test_already_escaped(self):
        """Already escaped quotes should not be double-escaped."""
        assert escape_single_quotes("it\\'s") == "it\\'s"


class TestParseAction:
    """Test action string parsing."""

    def test_parse_click_action(self):
        """Parse click action with coordinates."""
        result = parse_action("click(start_box='<|box_start|>(100,200)<|box_end|>')")
        assert result is not None
        assert result["function"] == "click"
        assert "start_box" in result["args"]

    def test_parse_type_action(self):
        """Parse type action with content."""
        result = parse_action("type(content='hello world')")
        assert result is not None
        assert result["function"] == "type"
        assert result["args"]["content"] == "hello world"

    def test_parse_hotkey_action(self):
        """Parse hotkey action."""
        result = parse_action("hotkey(key='ctrl c')")
        assert result is not None
        assert result["function"] == "hotkey"
        assert result["args"]["key"] == "ctrl c"

    def test_parse_scroll_action(self):
        """Parse scroll action with direction."""
        result = parse_action("scroll(start_box='(100,200)', direction='down')")
        assert result is not None
        assert result["function"] == "scroll"
        assert result["args"]["direction"] == "down"

    def test_parse_drag_action(self):
        """Parse drag action with start and end boxes."""
        result = parse_action("drag(start_box='(100,200)', end_box='(300,400)')")
        assert result is not None
        assert result["function"] == "drag"
        assert "start_box" in result["args"]
        assert "end_box" in result["args"]

    def test_parse_finished_action(self):
        """Parse finished action with content."""
        result = parse_action("finished(content='Task completed')")
        assert result is not None
        assert result["function"] == "finished"
        assert result["args"]["content"] == "Task completed"

    def test_parse_wait_action(self):
        """Parse wait action without arguments."""
        result = parse_action("wait()")
        assert result is not None
        assert result["function"] == "wait"
        assert result["args"] == {}

    def test_parse_invalid_action(self):
        """Invalid action strings should return None."""
        result = parse_action("not a valid action")
        assert result is None

    def test_parse_malformed_action(self):
        """Malformed action strings should return None."""
        result = parse_action("click(start_box=")
        assert result is None


class TestParseUITARSResponse:
    """Test UITARS response parsing."""

    def test_parse_thought_and_action(self):
        """Parse response with both thought and action."""
        text = "Thought: I need to click the button\nAction: click(start_box='(100,200)')"
        result = parse_uitars_response(text, 1024, 768)

        assert len(result) == 1
        assert result[0]["thought"] == "I need to click the button"
        assert result[0]["action_type"] == "click"

    def test_parse_action_only(self):
        """Parse response with only action."""
        text = "Action: type(content='hello')"
        result = parse_uitars_response(text, 1024, 768)

        assert len(result) == 1
        assert result[0]["thought"] is None
        assert result[0]["action_type"] == "type"

    def test_parse_without_action_raises_error(self):
        """Response without action should raise ValueError."""
        text = "Thought: I need to do something"
        with pytest.raises(ValueError, match="No Action found"):
            parse_uitars_response(text, 1024, 768)

    def test_parse_coordinates_normalization(self):
        """Coordinates should be normalized to 0-1 range."""
        text = "Action: click(start_box='(500,300)')"
        result = parse_uitars_response(text, 1024, 768)

        # 500/1000 = 0.5, 300/1000 = 0.3
        coords = eval(result[0]["action_inputs"]["start_box"])
        assert coords[0] == pytest.approx(0.5, rel=0.01)
        assert coords[1] == pytest.approx(0.3, rel=0.01)


class TestConvertToComputerActions:
    """Test conversion of parsed responses to computer actions."""

    def test_convert_click_action(self):
        """Convert click action to computer action."""
        parsed = [{
            "action_type": "click",
            "action_inputs": {"start_box": "[0.25, 0.25, 0.25, 0.25]"},
            "thought": None,
            "text": ""
        }]
        # 0.25 * 1024 = 256, 0.25 * 768 = 192
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "click"
        assert actions[0]["action"]["x"] == 256
        assert actions[0]["action"]["y"] == 192

    def test_convert_double_click_action(self):
        """Convert double_click action to computer action."""
        parsed = [{
            "action_type": "double_click",
            "action_inputs": {"start_box": "[0.5, 0.5, 0.5, 0.5]"},
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "double_click"

    def test_convert_type_action(self):
        """Convert type action to computer action."""
        parsed = [{
            "action_type": "type",
            "action_inputs": {"content": "hello world"},
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "type"
        assert actions[0]["action"]["text"] == "hello world"

    def test_convert_scroll_action(self):
        """Convert scroll action to computer action."""
        parsed = [{
            "action_type": "scroll",
            "action_inputs": {
                "start_box": "[0.5, 0.5, 0.5, 0.5]",
                "direction": "down"
            },
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "scroll"
        assert actions[0]["action"]["scroll_y"] == -5  # down direction

    def test_convert_scroll_up_action(self):
        """Convert scroll up action."""
        parsed = [{
            "action_type": "scroll",
            "action_inputs": {
                "start_box": "[0.5, 0.5, 0.5, 0.5]",
                "direction": "up"
            },
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert actions[0]["action"]["scroll_y"] == 5  # up direction

    def test_convert_drag_action(self):
        """Convert drag action with start and end coordinates."""
        parsed = [{
            "action_type": "drag",
            "action_inputs": {
                "start_box": "[0.1, 0.1, 0.1, 0.1]",
                "end_box": "[0.9, 0.9, 0.9, 0.9]"
            },
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "drag"
        # Check path coordinates
        path = actions[0]["action"]["path"]
        assert len(path) == 2

    def test_convert_hotkey_action(self):
        """Convert hotkey action."""
        parsed = [{
            "action_type": "hotkey",
            "action_inputs": {"key": "ctrl c"},
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "keypress"
        assert actions[0]["action"]["keys"] == ["ctrl", "c"]

    def test_convert_wait_action(self):
        """Convert wait action."""
        parsed = [{
            "action_type": "wait",
            "action_inputs": {},
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        assert actions[0]["action"]["type"] == "wait"

    def test_convert_finished_action(self):
        """Convert finished action should break loop."""
        parsed = [{
            "action_type": "finished",
            "action_inputs": {"content": "All done!"},
            "thought": None,
            "text": ""
        }]
        actions = convert_to_computer_actions(parsed, 1024, 768)

        assert len(actions) == 1
        # Finished should output text
        assert actions[0]["type"] == "message"


class TestConvertUITARSMessagesToLiteLLM:
    """Test conversion of UITARS internal messages to LiteLLM format."""

    def test_convert_reasoning_message(self):
        """Convert reasoning message to LiteLLM format."""
        messages = [{
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "I need to click the button"}]
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) >= 1
        # Reasoning should be converted to assistant message
        assert result[0]["role"] == "assistant"
        assert "I need to click the button" in result[0]["content"][0]["text"]

    def test_convert_computer_call_click(self):
        """Convert computer_call with click action."""
        messages = [{
            "type": "computer_call",
            "action": {"type": "click", "x": 100, "y": 200, "button": "left"}
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "click" in result[0]["content"][0]["text"]

    def test_convert_computer_call_double_click(self):
        """Convert computer_call with double_click action."""
        messages = [{
            "type": "computer_call",
            "action": {"type": "double_click", "x": 100, "y": 200}
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "left_double" in result[0]["content"][0]["text"]

    def test_convert_computer_call_type(self):
        """Convert computer_call with type action."""
        messages = [{
            "type": "computer_call",
            "action": {"type": "type", "text": "hello world"}
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "type" in result[0]["content"][0]["text"]
        assert "hello world" in result[0]["content"][0]["text"]

    def test_convert_computer_call_drag(self):
        """Convert computer_call with drag action."""
        messages = [{
            "type": "computer_call",
            "action": {
                "type": "drag",
                "start_x": 100, "start_y": 200,
                "end_x": 300, "end_y": 400
            }
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "drag" in result[0]["content"][0]["text"]

    def test_convert_computer_call_scroll(self):
        """Convert computer_call with scroll action."""
        messages = [{
            "type": "computer_call",
            "action": {"type": "scroll", "x": 100, "y": 200, "direction": "down"}
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "scroll" in result[0]["content"][0]["text"]

    def test_convert_computer_call_output(self):
        """Convert computer_call_output with screenshot."""
        messages = [{
            "type": "computer_call_output",
            "output": {
                "type": "input_image",
                "image_url": "data:image/png;base64,abc123"
            }
        }]
        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"][0]["type"] == "image_url"

    def test_convert_multiple_messages(self):
        """Convert multiple messages in sequence."""
        messages = [
            {"type": "reasoning", "summary": [{"type": "summary_text", "text": "Step 1"}]},
            {"type": "computer_call", "action": {"type": "click", "x": 100, "y": 200}},
            {"type": "computer_call_output", "output": {"type": "input_image", "image_url": "data:image/png;base64,test"}},
        ]
        result = convert_uitars_messages_to_litellm(messages)

        # Should have reasoning + action as assistant, then screenshot as user
        assert len(result) >= 2

    def test_convert_empty_messages(self):
        """Convert empty message list."""
        result = convert_uitars_messages_to_litellm([])
        assert result == []


class TestMessageOrdering:
    """Test message ordering in UITARS loop."""

    def test_message_role_ordering(self):
        """Verify messages are ordered: System -> History -> Current."""
        # This tests the expected order in predict_step
        messages = [
            {"role": "user", "content": "Take a screenshot"},
            {"type": "computer_call", "action": {"type": "click", "x": 100, "y": 200}},
            {"type": "computer_call_output", "output": {"type": "input_image", "image_url": "data:image/png;base64,test"}},
        ]

        result = convert_uitars_messages_to_litellm(messages)

        # All messages should have valid roles
        for msg in result:
            assert "role" in msg
            assert msg["role"] in ["user", "assistant", "system"]


class TestEdgeCases:
    """Test edge cases and error handling."""

    def test_empty_history(self):
        """Handle empty message history gracefully."""
        result = convert_uitars_messages_to_litellm([])
        assert result == []

    def test_single_message(self):
        """Handle single message."""
        messages = [{"type": "reasoning", "summary": [{"type": "summary_text", "text": "Think"}]}]
        result = convert_uitars_messages_to_litellm(messages)
        assert len(result) == 1

    def test_type_with_special_characters(self):
        """Handle type action with special characters."""
        text = "Action: type(content='hello\\nworld')"
        result = parse_uitars_response(text, 1024, 768)
        assert result[0]["action_type"] == "type"

    def test_finished_with_escape_characters(self):
        """Handle finished action with escape characters."""
        text = "Action: finished(content='It\\'s done!')"
        result = parse_uitars_response(text, 1024, 768)
        assert result[0]["action_type"] == "finished"

    def test_unknown_action_type(self):
        """Handle unknown action types gracefully."""
        parsed = [{
            "action_type": "unknown_action",
            "action_inputs": {},
            "thought": None,
            "text": ""
        }]
        # Should not raise, just return empty list
        actions = convert_to_computer_actions(parsed, 1024, 768)
        assert actions == []