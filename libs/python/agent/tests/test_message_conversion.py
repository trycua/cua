"""Unit tests for message conversion functions across all agent loops.

This file tests the message conversion utilities used by all agent loops.
Following SRP: This file tests ONLY message conversion functionality.
All external dependencies are mocked.
"""

import pytest
import json
from unittest.mock import MagicMock, patch


class TestConvertResponsesItemsToCompletionMessages:
    """Test conversion from responses_items format to completion messages format."""

    def test_convert_user_text_message(self):
        """Convert simple user text message."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{"role": "user", "content": "Hello, world!"}]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"] == "Hello, world!"

    def test_convert_user_image_message(self):
        """Convert user message with image."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": "data:image/png;base64,abc123"
            }]
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"][0]["type"] == "image_url"

    def test_convert_assistant_text_message(self):
        """Convert assistant text message."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "role": "assistant",
            "content": [{"type": "output_text", "text": "Hello!"}]
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert result[0]["content"] == "Hello!"

    def test_convert_function_call(self):
        """Convert function call to tool_calls format."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "function_call",
            "call_id": "call_123",
            "name": "get_weather",
            "arguments": '{"location": "San Francisco"}'
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "tool_calls" in result[0]
        assert len(result[0]["tool_calls"]) == 1
        assert result[0]["tool_calls"][0]["function"]["name"] == "get_weather"

    def test_convert_computer_call(self):
        """Convert computer call to tool_calls format."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "computer_call",
            "call_id": "call_456",
            "action": {"type": "click", "x": 100, "y": 200}
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "tool_calls" in result[0]
        assert result[0]["tool_calls"][0]["function"]["name"] == "computer"

    def test_convert_function_call_output(self):
        """Convert function call output to tool message."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "function_call_output",
            "call_id": "call_123",
            "output": "Sunny, 72°F"
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "tool"
        assert result[0]["tool_call_id"] == "call_123"
        assert result[0]["content"] == "Sunny, 72°F"

    def test_convert_computer_call_output_with_image(self):
        """Convert computer call output with screenshot."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "computer_call_output",
            "call_id": "call_456",
            "output": {
                "type": "input_image",
                "image_url": "data:image/png;base64,screenshot"
            }
        }]
        result = convert_responses_items_to_completion_messages(messages)

        # Should have tool message and user message with image
        assert len(result) >= 1

    def test_convert_reasoning_item(self):
        """Convert reasoning item to assistant message."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "Let me think..."}]
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "Let me think..." in result[0]["content"]

    def test_convert_mixed_content(self):
        """Convert message with mixed text and image content."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "What's in this image?"},
                {"type": "input_image", "image_url": "data:image/png;base64,img"}
            ]
        }]
        result = convert_responses_items_to_completion_messages(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert len(result[0]["content"]) == 2


class TestConvertCompletionMessagesToResponsesItems:
    """Test conversion from completion messages format to responses_items format."""

    def test_convert_user_message(self):
        """Convert user message to responses format."""
        from agent.responses import convert_completion_messages_to_responses_items

        messages = [{"role": "user", "content": "Hello!"}]
        result = convert_completion_messages_to_responses_items(messages)

        assert len(result) == 1
        assert result[0]["role"] == "user"
        assert result[0]["content"] == "Hello!"

    def test_convert_assistant_message(self):
        """Convert assistant message to responses format."""
        from agent.responses import convert_completion_messages_to_responses_items

        messages = [{"role": "assistant", "content": "Hi there!"}]
        result = convert_completion_messages_to_responses_items(messages)

        assert len(result) == 1
        assert result[0]["type"] == "message"
        assert result[0]["role"] == "assistant"

    def test_convert_tool_calls(self):
        """Convert tool calls to responses format."""
        from agent.responses import convert_completion_messages_to_responses_items

        messages = [{
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "arguments": '{"location": "SF"}'
                }
            }]
        }]
        result = convert_completion_messages_to_responses_items(messages)

        # Should have assistant message and function call
        assert len(result) >= 1
        function_call = [r for r in result if r.get("type") == "function_call"]
        assert len(function_call) == 1
        assert function_call[0]["name"] == "get_weather"

    def test_convert_computer_tool_call(self):
        """Convert computer tool call to responses format."""
        from agent.responses import convert_completion_messages_to_responses_items

        messages = [{
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_456",
                "type": "function",
                "function": {
                    "name": "computer",
                    "arguments": '{"type": "click", "x": 100, "y": 200}'
                }
            }]
        }]
        result = convert_completion_messages_to_responses_items(messages)

        computer_call = [r for r in result if r.get("type") == "computer_call"]
        assert len(computer_call) == 1

    def test_convert_tool_message(self):
        """Convert tool message to responses format."""
        from agent.responses import convert_completion_messages_to_responses_items

        messages = [{
            "role": "tool",
            "tool_call_id": "call_123",
            "content": "Tool result"
        }]
        result = convert_completion_messages_to_responses_items(messages)

        assert len(result) == 1
        assert result[0]["type"] == "function_call_output"
        assert result[0]["call_id"] == "call_123"


class TestConvertComputerCallsDesc2Xy:
    """Test conversion from element descriptions to coordinates."""

    def test_convert_single_description(self):
        """Convert single element description to coordinates."""
        from agent.responses import convert_computer_calls_desc2xy

        items = [{
            "type": "computer_call",
            "action": {
                "type": "click",
                "element_description": "Submit button"
            }
        }]
        desc2xy = {"Submit button": (100, 200)}

        result = convert_computer_calls_desc2xy(items, desc2xy)

        assert result[0]["action"]["x"] == 100
        assert result[0]["action"]["y"] == 200
        assert "element_description" not in result[0]["action"]

    def test_convert_drag_descriptions(self):
        """Convert drag start and end descriptions."""
        from agent.responses import convert_computer_calls_desc2xy

        items = [{
            "type": "computer_call",
            "action": {
                "type": "drag",
                "start_element_description": "File icon",
                "end_element_description": "Trash folder"
            }
        }]
        desc2xy = {
            "File icon": (100, 200),
            "Trash folder": (300, 400)
        }

        result = convert_computer_calls_desc2xy(items, desc2xy)

        assert "path" in result[0]["action"]
        assert len(result[0]["action"]["path"]) == 2

    def test_non_computer_call_unchanged(self):
        """Non-computer call items should pass through unchanged."""
        from agent.responses import convert_computer_calls_desc2xy

        items = [{"type": "message", "content": "Hello"}]
        result = convert_computer_calls_desc2xy(items, {})

        assert result == items


class TestConvertComputerCallsXy2Desc:
    """Test conversion from coordinates to element descriptions."""

    def test_convert_single_coordinate(self):
        """Convert single coordinate to element description."""
        from agent.responses import convert_computer_calls_xy2desc

        items = [{
            "type": "computer_call",
            "action": {
                "type": "click",
                "x": 100,
                "y": 200
            }
        }]
        desc2xy = {"Submit button": (100, 200)}

        result = convert_computer_calls_xy2desc(items, desc2xy)

        assert result[0]["action"]["element_description"] == "Submit button"
        assert "x" not in result[0]["action"]
        assert "y" not in result[0]["action"]

    def test_convert_drag_path(self):
        """Convert drag path to start/end descriptions."""
        from agent.responses import convert_computer_calls_xy2desc

        items = [{
            "type": "computer_call",
            "action": {
                "type": "drag",
                "path": [
                    {"x": 100, "y": 200},
                    {"x": 300, "y": 400}
                ]
            }
        }]
        desc2xy = {
            "File icon": (100, 200),
            "Trash folder": (300, 400)
        }

        result = convert_computer_calls_xy2desc(items, desc2xy)

        assert result[0]["action"]["start_element_description"] == "File icon"
        assert result[0]["action"]["end_element_description"] == "Trash folder"


class TestGetAllElementDescriptions:
    """Test extraction of all element descriptions."""

    def test_extract_single_description(self):
        """Extract single element description."""
        from agent.responses import get_all_element_descriptions

        items = [{
            "type": "computer_call",
            "action": {
                "type": "click",
                "element_description": "Submit button"
            }
        }]

        result = get_all_element_descriptions(items)
        assert "Submit button" in result

    def test_extract_multiple_descriptions(self):
        """Extract multiple element descriptions."""
        from agent.responses import get_all_element_descriptions

        items = [
            {
                "type": "computer_call",
                "action": {
                    "type": "click",
                    "element_description": "Button A"
                }
            },
            {
                "type": "computer_call",
                "action": {
                    "type": "click",
                    "element_description": "Button B"
                }
            }
        ]

        result = get_all_element_descriptions(items)
        assert len(result) == 2
        assert "Button A" in result
        assert "Button B" in result

    def test_extract_drag_descriptions(self):
        """Extract start and end descriptions from drag action."""
        from agent.responses import get_all_element_descriptions

        items = [{
            "type": "computer_call",
            "action": {
                "type": "drag",
                "start_element_description": "Source",
                "end_element_description": "Destination"
            }
        }]

        result = get_all_element_descriptions(items)
        assert len(result) == 2
        assert "Source" in result
        assert "Destination" in result


class TestReplaceFailedComputerCalls:
    """Test replacement of failed computer calls with function calls."""

    def test_replace_failed_call(self):
        """Replace computer call with function call when output indicates failure."""
        from agent.responses import replace_failed_computer_calls_with_function_calls

        messages = [
            {
                "type": "computer_call",
                "call_id": "call_123",
                "action": {"type": "click", "x": 100, "y": 200}
            },
            {
                "type": "function_call_output",
                "call_id": "call_123",
                "output": '{"error": "Click failed"}'
            }
        ]

        result = replace_failed_computer_calls_with_function_calls(messages)

        # Computer call should be replaced with function call
        computer_calls = [m for m in result if m.get("type") == "computer_call"]
        function_calls = [m for m in result if m.get("type") == "function_call"]

        assert len(computer_calls) == 0
        assert len(function_calls) == 1
        assert function_calls[0]["name"] == "computer"

    def test_keep_successful_call(self):
        """Keep computer call when there's no failure output."""
        from agent.responses import replace_failed_computer_calls_with_function_calls

        messages = [
            {
                "type": "computer_call",
                "call_id": "call_456",
                "action": {"type": "click", "x": 100, "y": 200}
            }
        ]

        result = replace_failed_computer_calls_with_function_calls(messages)

        computer_calls = [m for m in result if m.get("type") == "computer_call"]
        assert len(computer_calls) == 1


class TestRoundTripConversion:
    """Test round-trip conversion between formats."""

    def test_user_message_round_trip(self):
        """Test user message survives round-trip conversion."""
        from agent.responses import (
            convert_responses_items_to_completion_messages,
            convert_completion_messages_to_responses_items,
        )

        original = [{"role": "user", "content": "Hello, world!"}]

        # Convert to completion format and back
        completion = convert_responses_items_to_completion_messages(original)
        back = convert_completion_messages_to_responses_items(completion)

        assert back[0]["role"] == "user"
        assert "Hello, world!" in back[0].get("content", "")

    def test_function_call_round_trip(self):
        """Test function call survives round-trip conversion."""
        from agent.responses import (
            convert_responses_items_to_completion_messages,
            convert_completion_messages_to_responses_items,
        )

        original = [{
            "type": "function_call",
            "call_id": "call_123",
            "name": "test_function",
            "arguments": '{"arg": "value"}'
        }]

        completion = convert_responses_items_to_completion_messages(original)
        back = convert_completion_messages_to_responses_items(completion)

        function_calls = [b for b in back if b.get("type") == "function_call"]
        assert len(function_calls) == 1
        assert function_calls[0]["name"] == "test_function"


class TestXmlToolsMode:
    """Test XML tools mode for message conversion."""

    def test_xml_function_call_format(self):
        """Test function call in XML format."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "function_call",
            "call_id": "call_123",
            "name": "test",
            "arguments": '{"key": "value"}'
        }]

        result = convert_responses_items_to_completion_messages(
            messages,
            use_xml_tools=True,
            allow_images_in_tool_results=False
        )

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        # XML format includes function name
        assert "test" in result[0]["content"]

    def test_xml_computer_call_format(self):
        """Test computer call in XML format."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "computer_call",
            "call_id": "call_456",
            "action": {"type": "click", "x": 100, "y": 200}
        }]

        result = convert_responses_items_to_completion_messages(
            messages,
            use_xml_tools=True,
            allow_images_in_tool_results=False
        )

        assert len(result) == 1
        assert result[0]["role"] == "assistant"
        assert "computer" in result[0]["content"]

    def test_xml_tool_output_as_user_message(self):
        """Test tool output sent as user message in XML mode."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "function_call_output",
            "call_id": "call_123",
            "output": "Result"
        }]

        result = convert_responses_items_to_completion_messages(
            messages,
            use_xml_tools=True,
            allow_images_in_tool_results=False
        )

        assert len(result) == 1
        assert result[0]["role"] == "user"


class TestEdgeCases:
    """Test edge cases in message conversion."""

    def test_empty_messages_list(self):
        """Test handling of empty messages list."""
        from agent.responses import convert_responses_items_to_completion_messages

        result = convert_responses_items_to_completion_messages([])
        assert result == []

    def test_none_content(self):
        """Test handling of None content."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{"role": "user", "content": None}]
        result = convert_responses_items_to_completion_messages(messages)

        # Should handle gracefully
        assert isinstance(result, list)

    def test_empty_content_list(self):
        """Test handling of empty content list."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{"role": "user", "content": []}]
        result = convert_responses_items_to_completion_messages(messages)

        assert isinstance(result, list)

    def test_dict_arguments_in_function_call(self):
        """Test function call with dict arguments (should be JSON stringified)."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{
            "type": "function_call",
            "call_id": "call_123",
            "name": "test",
            "arguments": {"key": "value"}  # Dict instead of string
        }]

        result = convert_responses_items_to_completion_messages(messages)

        # Should convert dict to JSON string
        args = result[0]["tool_calls"][0]["function"]["arguments"]
        assert isinstance(args, str)

    def test_unknown_message_type(self):
        """Test handling of unknown message type."""
        from agent.responses import convert_responses_items_to_completion_messages

        messages = [{"type": "unknown_type", "data": "test"}]
        result = convert_responses_items_to_completion_messages(messages)

        # Should pass through or be skipped
        assert isinstance(result, list)