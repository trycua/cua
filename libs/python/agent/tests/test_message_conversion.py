"""Unit tests for shared message conversion utilities in responses.py.

Tests the convert_responses_items_to_completion_messages and
convert_completion_messages_to_responses_items functions which provide
generic message format conversion used by multiple agent loops.
"""

import json

import pytest
from agent.responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
)


class TestConvertResponsesToCompletion:
    """Tests for convert_responses_items_to_completion_messages function."""

    def test_empty_messages_returns_empty_list(self):
        """Empty input should return empty output."""
        result = convert_responses_items_to_completion_messages([])
        assert result == []

    # --- User message tests ---

    def test_user_message_with_string_content(self):
        """User message with string content should be preserved."""
        messages = [{"role": "user", "content": "Hello, world!"}]

        result = convert_responses_items_to_completion_messages(messages)

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

        result = convert_responses_items_to_completion_messages(messages)

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

        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["content"][0]["type"] == "text"
        assert result[0]["content"][0]["text"] == "Some text"

    # --- Assistant message tests ---

    def test_assistant_message_with_output_text(self):
        """Assistant message with output_text should have text joined."""
        messages = [
            {
                "role": "assistant",
                "type": "message",
                "content": [
                    {"type": "output_text", "text": "First part"},
                    {"type": "output_text", "text": "Second part"},
                ],
            }
        ]

        result = convert_responses_items_to_completion_messages(messages)

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
                "summary": [{"type": "summary_text", "text": "Thinking about this..."}],
            }
        ]

        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert result[0]["content"] == "Thinking about this..."

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

        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "tool_calls" in result[0]
        tool_call = result[0]["tool_calls"][0]
        assert tool_call["id"] == "call_123"
        assert tool_call["function"]["name"] == "get_weather"

    def test_function_call_output_creates_tool_message(self):
        """Function call output should create tool role message."""
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

        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 2
        assert result[1]["role"] == "tool"
        assert result[1]["tool_call_id"] == "call_123"
        assert result[1]["content"] == "Sunny, 72°F"

    # --- Computer call tests ---

    def test_computer_call_creates_tool_call(self):
        """Computer call should create assistant message with tool_calls."""
        messages = [
            {
                "type": "computer_call",
                "call_id": "call_1",
                "action": {"type": "click", "x": 100, "y": 200},
            }
        ]

        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "tool_calls" in result[0]
        tool_call = result[0]["tool_calls"][0]
        assert tool_call["function"]["name"] == "computer"
        args = json.loads(tool_call["function"]["arguments"])
        assert args["type"] == "click"

    def test_computer_call_output_with_image_allows_in_tool_results(self):
        """Computer call output with image should be in tool message when allowed."""
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

        result = convert_responses_items_to_completion_messages(
            messages, allow_images_in_tool_results=True
        )

        assert len(result) == 2
        assert result[1]["role"] == "tool"
        assert result[1]["content"][0]["type"] == "image_url"

    def test_computer_call_output_with_image_separate_user_message(self):
        """Computer call output with image should create separate user message when not allowed."""
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

        result = convert_responses_items_to_completion_messages(
            messages, allow_images_in_tool_results=False
        )

        # Should have: assistant (tool_call), tool (text), user (image)
        assert len(result) == 3
        assert result[1]["role"] == "tool"
        assert "[Execution completed" in result[1]["content"]
        assert result[2]["role"] == "user"
        assert result[2]["content"][0]["type"] == "image_url"

    # --- XML tools mode tests ---

    def test_xml_tools_mode_function_call(self):
        """Function call in XML mode should use <tool_call> tags."""
        messages = [
            {
                "type": "function_call",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": '{"city": "NYC"}',
            }
        ]

        result = convert_responses_items_to_completion_messages(
            messages, allow_images_in_tool_results=False, use_xml_tools=True
        )

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "<tool_call>" in result[0]["content"]
        assert "get_weather" in result[0]["content"]

    def test_xml_tools_mode_function_output_as_user(self):
        """Function output in XML mode should be user message."""
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
                "output": "Sunny",
            },
        ]

        result = convert_responses_items_to_completion_messages(
            messages, allow_images_in_tool_results=False, use_xml_tools=True
        )

        assert len(result) == 2
        assert result[1]["role"] == "user"
        assert result[1]["content"] == "Sunny"


class TestConvertCompletionToResponses:
    """Tests for convert_completion_messages_to_responses_items function."""

    def test_empty_messages_returns_empty_list(self):
        """Empty input should return empty output."""
        result = convert_completion_messages_to_responses_items([])
        assert result == []

    def test_user_message_with_string_content(self):
        """User message with string content should be preserved."""
        messages = [{"role": "user", "content": "Hello"}]

        result = convert_completion_messages_to_responses_items(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"] == "Hello"

    def test_user_message_with_image_url(self):
        """User message with image_url should be converted to input_image."""
        image_url = "data:image/png;base64,abc123"
        messages = [
            {
                "role": "user",
                "content": [{"type": "image_url", "image_url": {"url": image_url}}],
            }
        ]

        result = convert_completion_messages_to_responses_items(messages)

        assert len(result) == 1
        assert result[0]["content"][0]["type"] == "input_image"
        assert result[0]["content"][0]["image_url"] == image_url

    def test_assistant_message_with_text(self):
        """Assistant message with text should be converted to output_text."""
        messages = [{"role": "assistant", "content": "I can help you"}]

        result = convert_completion_messages_to_responses_items(messages)

        assert len(result) == 1
        assert result[0]["type"] == "message"
        assert result[0]["role"] == "assistant"
        assert result[0]["content"][0]["type"] == "output_text"

    def test_tool_call_to_function_call(self):
        """Tool call should be converted to function_call item."""
        messages = [
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_123",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": '{"city": "NYC"}'},
                    }
                ],
            }
        ]

        result = convert_completion_messages_to_responses_items(messages)

        # Should have function_call item
        function_calls = [r for r in result if r.get("type") == "function_call"]
        assert len(function_calls) == 1
        assert function_calls[0]["name"] == "get_weather"
        assert function_calls[0]["call_id"] == "call_123"

    def test_computer_tool_call_to_computer_call(self):
        """Computer tool call should be converted to computer_call item."""
        messages = [
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "computer",
                            "arguments": json.dumps({"action": "click", "x": 100, "y": 200}),
                        },
                    }
                ],
            }
        ]

        result = convert_completion_messages_to_responses_items(messages)

        # Should have computer_call item
        computer_calls = [r for r in result if r.get("type") == "computer_call"]
        assert len(computer_calls) == 1
        assert computer_calls[0]["action"]["type"] == "click"

    def test_tool_message_with_screenshot_pattern(self):
        """Tool message with 'see screenshot below' pattern should be handled."""
        image_url = "data:image/png;base64,abc"
        messages = [
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "[Execution completed. See screenshot below]",
            },
            {
                "role": "user",
                "content": [{"type": "image_url", "image_url": {"url": image_url}}],
            },
        ]

        result = convert_completion_messages_to_responses_items(messages)

        # Should detect the pattern and create computer_call_output with image
        outputs = [r for r in result if r.get("type") == "computer_call_output"]
        assert len(outputs) == 1
        assert outputs[0]["output"]["type"] == "input_image"
        assert outputs[0]["output"]["image_url"] == image_url

    def test_tool_message_with_text_output(self):
        """Tool message with plain text should become function_call_output."""
        messages = [
            {
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "Result: 42",
            }
        ]

        result = convert_completion_messages_to_responses_items(messages)

        outputs = [r for r in result if r.get("type") == "function_call_output"]
        assert len(outputs) == 1
        assert outputs[0]["output"] == "Result: 42"


class TestRoundTripConversion:
    """Tests for round-trip conversion between formats."""

    def test_user_message_round_trip(self):
        """User message should survive round-trip conversion."""
        original = [{"role": "user", "content": "Hello, world!"}]

        completion = convert_responses_items_to_completion_messages(original)
        responses = convert_completion_messages_to_responses_items(completion)

        assert responses[0]["role"] == "user"
        assert responses[0]["content"] == "Hello, world!"

    def test_function_call_round_trip(self):
        """Function call should survive round-trip conversion."""
        original = [
            {
                "type": "function_call",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": '{"city": "NYC"}',
            }
        ]

        completion = convert_responses_items_to_completion_messages(original)
        responses = convert_completion_messages_to_responses_items(completion)

        # Should have function_call
        function_calls = [r for r in responses if r.get("type") == "function_call"]
        assert len(function_calls) == 1
        assert function_calls[0]["name"] == "get_weather"
