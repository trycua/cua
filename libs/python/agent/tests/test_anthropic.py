"""Unit tests for Anthropic agent loop message conversion.

Tests the _convert_responses_items_to_completion_messages function which converts
responses_items message format to LiteLLM completion format for Anthropic API.

Intended behavior:
- User messages converted with content (input_image -> image_url, input_text -> text)
- Assistant messages have content joined with newlines
- Reasoning messages become assistant messages
- Function calls become OpenAI tool_calls format
- Computer calls converted to Anthropic action names (click -> left_click, etc.)
- Computer call outputs become function role messages with image or text content
"""

import json

import pytest
from agent.loops.anthropic import _convert_responses_items_to_completion_messages


class TestConvertResponsesToCompletionMessages:
    """Tests for _convert_responses_items_to_completion_messages function."""

    def test_empty_messages_returns_empty_list(self):
        """Empty input should return empty output."""
        result = _convert_responses_items_to_completion_messages([])
        assert result == []

    # --- User message tests ---

    def test_user_message_with_string_content(self):
        """User message with string content should be preserved."""
        messages = [{"role": "user", "content": "Hello, world!"}]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"] == "Hello, world!"

    def test_user_message_with_input_image(self):
        """User message with input_image should be converted to image_url format."""
        image_url = "data:image/png;base64,abc123"
        messages = [
            {
                "role": "user",
                "content": [{"type": "input_image", "image_url": image_url}],
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"][0]["type"] == "image_url"
        assert result[0]["content"][0]["image_url"]["url"] == image_url

    def test_user_message_with_input_text(self):
        """User message with input_text should be converted to text format."""
        messages = [
            {
                "role": "user",
                "content": [{"type": "input_text", "text": "Some text"}],
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"][0]["type"] == "text"
        assert result[0]["content"][0]["text"] == "Some text"

    def test_user_message_omitted_image_url_drops_message(self):
        """User message with only omitted image_url should be dropped entirely."""
        messages = [
            {
                "role": "user",
                "content": [{"type": "input_image", "image_url": "[omitted]"}],
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        # Message with only filtered content should be dropped, not sent with empty content
        assert len(result) == 0

    # --- Assistant message tests ---

    def test_assistant_message_with_string_content(self):
        """Assistant message with string content should be preserved."""
        messages = [{"role": "assistant", "content": "I can help you with that."}]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        # String content is wrapped in output_text then joined back
        assert result[0]["content"] == "I can help you with that."

    def test_assistant_message_with_output_text_list(self):
        """Assistant message with output_text list should have text joined."""
        messages = [
            {
                "role": "assistant",
                "content": [
                    {"type": "output_text", "text": "First part"},
                    {"type": "output_text", "text": "Second part"},
                ],
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "First part" in result[0]["content"]
        assert "Second part" in result[0]["content"]

    # --- Reasoning message tests ---

    def test_reasoning_message_becomes_assistant(self):
        """Reasoning message should become assistant message."""
        messages = [
            {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "Analyzing the screen..."}],
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert result[0]["content"] == "Analyzing the screen..."

    def test_reasoning_message_with_empty_summary_skipped(self):
        """Reasoning message with empty summary should not create a message."""
        messages = [{"type": "reasoning", "summary": []}]

        result = _convert_responses_items_to_completion_messages(messages)

        assert result == []

    # --- Function call tests ---

    def test_function_call_creates_tool_call(self):
        """Function call should create assistant message with tool_calls."""
        messages = [
            {
                "type": "function_call",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": '{"city": "NYC"}',
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "tool_calls" in result[0]
        assert len(result[0]["tool_calls"]) == 1
        tool_call = result[0]["tool_calls"][0]
        assert tool_call["id"] == "call_123"
        assert tool_call["type"] == "function"
        assert tool_call["function"]["name"] == "get_weather"
        assert tool_call["function"]["arguments"] == '{"city": "NYC"}'

    def test_function_call_extends_existing_assistant(self):
        """Function call should extend existing assistant message's tool_calls."""
        messages = [
            {"role": "assistant", "content": "Let me check that."},
            {
                "type": "function_call",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": "{}",
            },
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        # Should be combined into one assistant message
        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "tool_calls" in result[0]

    def test_function_call_output(self):
        """Function call output should create function role message."""
        messages = [
            {
                "type": "function_call",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": "{}",
            },
            {
                "type": "function_call_output",
                "call_id": "call_123",
                "output": "Sunny, 72°F",
            },
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 2
        assert result[1]["role"] == "function"
        assert result[1]["tool_call_id"] == "call_123"
        assert result[1]["content"] == "Sunny, 72°F"

    # --- Computer call tests ---

    def test_computer_call_left_click(self):
        """Computer call with left click should become left_click tool_use."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 100, "y": 200, "button": "left"},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        tool_call = result[0]["tool_calls"][0]
        assert tool_call["function"]["name"] == "computer"
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "left_click"
        assert args["coordinate"] == [100, 200]

    def test_computer_call_right_click(self):
        """Computer call with right click should become right_click action."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 50, "y": 75, "button": "right"},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "right_click"

    def test_computer_call_middle_click(self):
        """Computer call with wheel button should become middle_click action."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 50, "y": 75, "button": "wheel"},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "middle_click"

    def test_computer_call_double_click(self):
        """Computer call with double_click should preserve action type."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "double_click", "x": 100, "y": 200},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "double_click"
        assert args["coordinate"] == [100, 200]

    def test_computer_call_type(self):
        """Computer call with type action should preserve text."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "type", "text": "hello world"},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "type"
        assert args["text"] == "hello world"

    def test_computer_call_keypress(self):
        """Computer call with keypress should convert keys to joined string."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "keypress", "keys": ["ctrl", "c"]},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "key"
        assert args["text"] == "ctrl+c"

    def test_computer_call_scroll_down(self):
        """Computer call with scroll down should set direction and amount."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "scroll", "x": 300, "y": 400, "scroll_x": 0, "scroll_y": -5},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "scroll"
        assert args["coordinate"] == [300, 400]
        assert args["scroll_direction"] == "down"
        assert args["scroll_amount"] == 5

    def test_computer_call_scroll_up(self):
        """Computer call with scroll up should set direction correctly."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "scroll", "x": 300, "y": 400, "scroll_x": 0, "scroll_y": 5},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["scroll_direction"] == "up"

    def test_computer_call_drag(self):
        """Computer call with drag should convert path to start/end coordinates."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {
                    "type": "drag",
                    "path": [{"x": 100, "y": 150}, {"x": 200, "y": 250}],
                },
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "left_click_drag"
        assert args["start_coordinate"] == [100, 150]
        assert args["end_coordinate"] == [200, 250]

    def test_computer_call_move(self):
        """Computer call with move action should become mouse_move."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "move", "x": 150, "y": 250},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "mouse_move"
        assert args["coordinate"] == [150, 250]

    def test_computer_call_wait(self):
        """Computer call with wait action should preserve action type."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "wait"},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "wait"

    def test_computer_call_screenshot(self):
        """Computer call with screenshot action should preserve action type."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "screenshot"},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "screenshot"

    # --- Computer call output tests ---

    def test_computer_call_output_with_image(self):
        """Computer call output with image should create function message with image content."""
        image_url = "data:image/png;base64,xyz123"
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "screenshot"},
            },
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": {"type": "input_image", "image_url": image_url},
            },
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 2
        assert result[1]["role"] == "function"
        assert result[1]["tool_call_id"] == "call_1"
        assert result[1]["content"][0]["type"] == "image_url"
        assert result[1]["content"][0]["image_url"]["url"] == image_url

    def test_computer_call_output_with_text(self):
        """Computer call output with text should create function message with string content."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "type", "text": "test"},
            },
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": "Action completed successfully",
            },
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        assert len(result) == 2
        assert result[1]["role"] == "function"
        assert result[1]["content"] == "Action completed successfully"

    # --- Integration tests ---

    def test_full_conversation_flow(self):
        """Full conversation with user, assistant reasoning, action, and result."""
        messages = [
            {"role": "user", "content": "Click the submit button"},
            {
                "type": "reasoning",
                "summary": [
                    {"type": "summary_text", "text": "I see a submit button at coordinates 500,300"}
                ],
            },
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 500, "y": 300, "button": "left"},
            },
            {
                "type": "computer_call_output",
                "call_id": "call_1",
                "output": {"type": "input_image", "image_url": "data:image/png;base64,result"},
            },
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        # Should have: user, assistant (reasoning + tool_calls merged), function (output)
        # Reasoning and computer_call get merged into same assistant message
        assert len(result) == 3
        assert result[0]["role"] == "user"
        assert result[1]["role"] == "assistant"
        assert "tool_calls" in result[1]
        assert result[1]["content"] == "I see a submit button at coordinates 500,300"
        assert result[2]["role"] == "function"

    def test_multiple_tool_calls_extend_same_assistant(self):
        """Multiple consecutive computer calls should extend the same assistant message."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 100, "y": 200, "button": "left"},
            },
            {
                "type": "computer_call",
                "call_id": "call_2",
                "action": {"type": "type", "text": "hello"},
            },
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        # Both calls should be in the same assistant message
        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert len(result[0]["tool_calls"]) == 2

    def test_left_mouse_down_action(self):
        """Computer call with left_mouse_down should create correct action."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "left_mouse_down", "x": 100, "y": 200},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "left_mouse_down"

    def test_left_mouse_up_action(self):
        """Computer call with left_mouse_up should create correct action."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "left_mouse_up", "x": 100, "y": 200},
            }
        ]

        result = _convert_responses_items_to_completion_messages(messages)

        tool_call = result[0]["tool_calls"][0]
        args = json.loads(tool_call["function"]["arguments"])
        assert args["action"] == "left_mouse_up"
