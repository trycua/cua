"""Unit tests for UITARS agent loop message conversion.

Tests the convert_uitars_messages_to_litellm function which converts
UITARS internal message format (responses_items) back to LiteLLM format.

Intended behavior:
- Reasoning messages become assistant messages with "Thought: {text}" prefix
- Computer calls become assistant messages with "Action: {action_format}"
- Computer call outputs with images become user messages with image_url
- User messages are currently skipped
- Assistant content is accumulated and finalized after computer_call or at end
"""

import pytest
from agent.loops.uitars import convert_uitars_messages_to_litellm


class TestConvertUITARSMessagesToLiteLLM:
    """Tests for convert_uitars_messages_to_litellm function."""

    def test_empty_messages_returns_empty_list(self):
        """Empty input should return empty output."""
        result = convert_uitars_messages_to_litellm([])
        assert result == []

    def test_reasoning_message_becomes_thought_in_assistant(self):
        """Reasoning messages should become assistant messages with 'Thought:' prefix."""
        messages = [
            {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "I need to click the button"}],
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert result[0]["content"][0]["type"] == "text"
        assert "Thought: I need to click the button" in result[0]["content"][0]["text"]

    def test_click_action_becomes_action_text(self):
        """Computer call with click action should become Action: click(...) text."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 100, "y": 200, "button": "left"},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "Action:" in result[0]["content"][0]["text"]
        assert "click" in result[0]["content"][0]["text"]
        assert "100" in result[0]["content"][0]["text"]
        assert "200" in result[0]["content"][0]["text"]

    def test_right_click_action_uses_right_single(self):
        """Right click should be converted to right_single action format."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 50, "y": 75, "button": "right"},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "right_single" in result[0]["content"][0]["text"]

    def test_double_click_action_uses_left_double(self):
        """Double click should be converted to left_double action format."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "double_click", "x": 100, "y": 200},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "left_double" in result[0]["content"][0]["text"]

    def test_type_action_format(self):
        """Type action should be converted to type(content='...') format."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "type", "text": "hello world"},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "type(content=" in result[0]["content"][0]["text"]
        assert "hello world" in result[0]["content"][0]["text"]

    def test_scroll_action_format(self):
        """Scroll action should include position and direction."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "scroll", "x": 300, "y": 400, "direction": "down"},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "scroll" in result[0]["content"][0]["text"]
        assert "down" in result[0]["content"][0]["text"]

    def test_wait_action_format(self):
        """Wait action should be converted to wait() format."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "wait"},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "wait()" in result[0]["content"][0]["text"]

    def test_hotkey_action_format(self):
        """Hotkey/key action should be converted to hotkey format."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "key", "key": "ctrl+c"},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert "hotkey" in result[0]["content"][0]["text"]
        assert "ctrl+c" in result[0]["content"][0]["text"]

    def test_computer_call_output_with_image_becomes_user_message(self):
        """Computer call output with image should become user message with image_url."""
        image_url = "data:image/png;base64,abc123"
        messages = [
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": {"type": "input_image", "image_url": image_url},
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"][0]["type"] == "image_url"
        assert result[0]["content"][0]["image_url"]["url"] == image_url

    def test_reasoning_and_action_combined(self):
        """Reasoning followed by computer call should create combined assistant message."""
        messages = [
            {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "I see a button"}],
            },
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 100, "y": 200, "button": "left"},
            },
        ]

        result = convert_uitars_messages_to_litellm(messages)

        # Should have one assistant message with both thought and action
        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        text = result[0]["content"][0]["text"]
        assert "Thought:" in text
        assert "I see a button" in text
        assert "Action:" in text

    def test_full_turn_sequence(self):
        """Full turn: reasoning + action + screenshot should produce assistant + user messages."""
        messages = [
            {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "Clicking the submit button"}],
            },
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 500, "y": 300, "button": "left"},
            },
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": {"type": "input_image", "image_url": "data:image/png;base64,xyz"},
            },
        ]

        result = convert_uitars_messages_to_litellm(messages)

        # Should have: assistant (thought+action), user (screenshot)
        assert len(result) == 2
        assert result[0]["role"] == "assistant"
        assert result[1]["role"] == "user"
        assert result[1]["content"][0]["type"] == "image_url"

    def test_user_messages_are_skipped(self):
        """User messages in the input should be skipped (current behavior)."""
        messages = [
            {"role": "user", "content": "Click the button"},
        ]

        result = convert_uitars_messages_to_litellm(messages)

        # User messages are currently passed/skipped in the implementation
        assert result == []

    def test_computer_call_output_without_image_is_skipped(self):
        """Computer call output without image should not create a message."""
        messages = [
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": "some text output",
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        # Non-image outputs don't create messages in current implementation
        assert result == []

    def test_drag_action_format(self):
        """Drag action should include start and end coordinates."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {
                    "type": "drag",
                    "start_x": 100,
                    "start_y": 150,
                    "end_x": 200,
                    "end_y": 250,
                },
            }
        ]

        result = convert_uitars_messages_to_litellm(messages)

        assert len(result) == 1
        text = result[0]["content"][0]["text"]
        assert "drag" in text

    def test_multiple_actions_create_multiple_assistant_messages(self):
        """Multiple computer calls should each finalize their own assistant message."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 100, "y": 200, "button": "left"},
            },
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": {"type": "input_image", "image_url": "data:image/png;base64,img1"},
            },
            {
                "type": "computer_call",
                "call_id": "call_2",
                "action": {"type": "type", "text": "hello"},
            },
            {
                "type": "computer_call_output",
                "call_id": "call_2",
                "output": {"type": "input_image", "image_url": "data:image/png;base64,img2"},
            },
        ]

        result = convert_uitars_messages_to_litellm(messages)

        # Should have: assistant, user, assistant, user
        assert len(result) == 4
        assert result[0]["role"] == "assistant"
        assert result[1]["role"] == "user"
        assert result[2]["role"] == "assistant"
        assert result[3]["role"] == "user"
