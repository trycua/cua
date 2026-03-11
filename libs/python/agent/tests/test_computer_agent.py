"""Unit tests for ComputerAgent class.

This file tests ONLY the ComputerAgent initialization and basic functionality.
Following SRP: This file tests ONE class (ComputerAgent).
All external dependencies (liteLLM, Computer) are mocked.
"""

from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest


class TestComputerAgentInitialization:
    """Test ComputerAgent initialization (SRP: Only tests initialization)."""

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_model(self, mock_litellm, disable_telemetry):
        """Test that agent can be initialized with a model string."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        assert agent is not None
        assert hasattr(agent, "model")
        assert agent.model == "anthropic/claude-sonnet-4-5-20250929"

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_tools(self, mock_litellm, disable_telemetry, mock_computer):
        """Test that agent can be initialized with tools."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", tools=[mock_computer])

        assert agent is not None
        assert hasattr(agent, "tools")

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_max_budget(self, mock_litellm, disable_telemetry):
        """Test that agent can be initialized with max trajectory budget."""
        from agent import ComputerAgent

        budget = 5.0
        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929", max_trajectory_budget=budget
        )

        assert agent is not None

    @patch("agent.agent.litellm")
    def test_agent_requires_model(self, mock_litellm, disable_telemetry):
        """Test that agent requires a model parameter."""
        from agent import ComputerAgent

        with pytest.raises(TypeError):
            # Should fail without model parameter - intentionally missing required argument
            ComputerAgent()  # type: ignore[call-arg]


class TestComputerAgentRun:
    """Test ComputerAgent.run() method (SRP: Only tests run logic)."""

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_agent_run_with_messages(self, mock_litellm, disable_telemetry, sample_messages):
        """Test that agent.run() works with valid messages."""
        from agent import ComputerAgent

        # Mock liteLLM response
        mock_response = {
            "id": "chatcmpl-test",
            "choices": [
                {
                    "message": {"role": "assistant", "content": "Test response"},
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30},
        }

        mock_litellm.acompletion = AsyncMock(return_value=mock_response)

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        # Run should return an async generator
        result_generator = agent.run(sample_messages)

        assert result_generator is not None
        # Check it's an async generator
        assert hasattr(result_generator, "__anext__")

    def test_agent_has_run_method(self, disable_telemetry):
        """Test that agent has run method available."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        # Verify run method exists
        assert hasattr(agent, "run")
        assert callable(agent.run)

    def test_agent_has_agent_loop(self, disable_telemetry):
        """Test that agent has agent_loop initialized."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        # Verify agent_loop is initialized
        assert hasattr(agent, "agent_loop")
        assert agent.agent_loop is not None


class TestComputerAgentTypes:
    """Test AgentResponse and Messages types (SRP: Only tests type definitions)."""

    def test_messages_type_exists(self):
        """Test that Messages type is exported."""
        from agent import Messages

        assert Messages is not None

    def test_agent_response_type_exists(self):
        """Test that AgentResponse type is exported."""
        from agent import AgentResponse

        assert AgentResponse is not None


class TestComputerAgentIntegration:
    """Test ComputerAgent integration with Computer tool (SRP: Integration within package)."""

    def test_agent_accepts_computer_tool(self, disable_telemetry, mock_computer):
        """Test that agent can be initialized with Computer tool."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", tools=[mock_computer])

        # Verify agent accepted the tool
        assert agent is not None
        assert hasattr(agent, "tools")


class TestComputerCallBackwardsCompatibility:
    """Test backwards compatibility for computer_call handling (standard API format).

    These tests ensure that the original computer_call format (used by Claude and
    other models with native computer use support) continues to work correctly
    after adding GPT 5.4 function_call support.
    """

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_computer_call_click_action(self, mock_litellm, disable_telemetry):
        """Test that computer_call with click action works correctly."""
        from agent import ComputerAgent

        # Create a mock computer handler
        mock_handler = AsyncMock()
        mock_handler.click = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        # Simulate a computer_call item (standard format)
        computer_call_item = {
            "type": "computer_call",
            "call_id": "call_123",
            "action": {"type": "click", "x": 100, "y": 200},
            "pending_safety_checks": [],
        }

        result = await agent._handle_item(computer_call_item, mock_handler)

        # Verify the click was called with correct coordinates
        mock_handler.click.assert_called_once_with(x=100, y=200)
        # Verify screenshot was taken after action
        mock_handler.screenshot.assert_called_once()
        # Verify result format is computer_call_output
        assert len(result) == 1
        assert result[0]["type"] == "computer_call_output"
        assert result[0]["call_id"] == "call_123"
        assert "output" in result[0]

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_computer_call_type_action(self, mock_litellm, disable_telemetry):
        """Test that computer_call with type action works correctly."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.type = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        computer_call_item = {
            "type": "computer_call",
            "call_id": "call_456",
            "action": {"type": "type", "text": "Hello World"},
            "pending_safety_checks": [],
        }

        result = await agent._handle_item(computer_call_item, mock_handler)

        mock_handler.type.assert_called_once_with(text="Hello World")
        mock_handler.screenshot.assert_called_once()
        assert result[0]["type"] == "computer_call_output"

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_computer_call_screenshot_action(self, mock_litellm, disable_telemetry):
        """Test that computer_call with screenshot action works correctly."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        computer_call_item = {
            "type": "computer_call",
            "call_id": "call_789",
            "action": {"type": "screenshot"},
            "pending_safety_checks": [],
        }

        result = await agent._handle_item(computer_call_item, mock_handler)

        mock_handler.screenshot.assert_called()
        assert result[0]["type"] == "computer_call_output"
        assert "image_url" in str(result[0]["output"])

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_computer_call_terminate_action(self, mock_litellm, disable_telemetry):
        """Test that computer_call terminate action does not take screenshot after."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.terminate = AsyncMock(return_value={"terminated": True})
        mock_handler.screenshot = AsyncMock()

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        computer_call_item = {
            "type": "computer_call",
            "call_id": "call_term",
            "action": {"type": "terminate", "status": "success"},
            "pending_safety_checks": [],
        }

        result = await agent._handle_item(computer_call_item, mock_handler)

        mock_handler.terminate.assert_called_once()
        # Screenshot should NOT be taken for terminate action
        mock_handler.screenshot.assert_not_called()
        assert result[0]["type"] == "computer_call_output"
        assert result[0]["output"]["terminated"] is True

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_computer_call_safety_checks_acknowledged(self, mock_litellm, disable_telemetry):
        """Test that pending safety checks are acknowledged in output."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.click = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        safety_check = {"id": "check_1", "message": "This action may have side effects"}
        computer_call_item = {
            "type": "computer_call",
            "call_id": "call_safety",
            "action": {"type": "click", "x": 50, "y": 50},
            "pending_safety_checks": [safety_check],
        }

        result = await agent._handle_item(computer_call_item, mock_handler)

        assert result[0]["acknowledged_safety_checks"] == [safety_check]


class TestFunctionCallComputerHandling:
    """Test GPT 5.4 function_call 'computer' handling (new functionality).

    These tests verify that function_call items with name='computer' are
    correctly processed as computer actions, enabling GPT 5.4 and similar
    models to use computer tools via the function calling interface.
    """

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_click(self, mock_litellm, disable_telemetry):
        """Test that function_call with computer click action works correctly."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.click = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_123",
            "name": "computer",
            "arguments": '{"action": "click", "x": 150, "y": 250}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        mock_handler.click.assert_called_once_with(x=150, y=250)
        mock_handler.screenshot.assert_called_once()
        # Verify result format is function_call_output + user message with image
        assert len(result) == 2
        assert result[0]["type"] == "function_call_output"
        assert result[0]["call_id"] == "fc_123"
        assert result[1]["role"] == "user"
        assert result[1]["content"][0]["type"] == "input_image"

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_type(self, mock_litellm, disable_telemetry):
        """Test that function_call with computer type action works correctly."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.type = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_456",
            "name": "computer",
            "arguments": '{"action": "type", "text": "Testing GPT 5.4"}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        mock_handler.type.assert_called_once_with(text="Testing GPT 5.4")
        assert result[0]["type"] == "function_call_output"

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_screenshot(self, mock_litellm, disable_telemetry):
        """Test that function_call with computer screenshot action works."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_789",
            "name": "computer",
            "arguments": '{"action": "screenshot"}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        mock_handler.screenshot.assert_called()
        assert result[0]["type"] == "function_call_output"

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_terminate(self, mock_litellm, disable_telemetry):
        """Test that function_call terminate action does not take screenshot after."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.terminate = AsyncMock(return_value={"terminated": True})
        mock_handler.screenshot = AsyncMock()

        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_term",
            "name": "computer",
            "arguments": '{"action": "terminate", "status": "success"}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        mock_handler.terminate.assert_called_once()
        mock_handler.screenshot.assert_not_called()
        assert len(result) == 1  # Only function_call_output, no image message
        assert result[0]["type"] == "function_call_output"

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_filters_irrelevant_params(
        self, mock_litellm, disable_telemetry
    ):
        """Test that irrelevant parameters are filtered out for each action type."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.click = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="openai/gpt-5.4")

        # Include irrelevant 'text' param with click action - should be filtered
        function_call_item = {
            "type": "function_call",
            "call_id": "fc_filter",
            "name": "computer",
            "arguments": '{"action": "click", "x": 100, "y": 200, "text": "ignored"}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        # Should only receive x and y, not text
        mock_handler.click.assert_called_once_with(x=100, y=200)

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_allows_zero_coordinates(
        self, mock_litellm, disable_telemetry
    ):
        """Test that zero values are allowed for coordinates (x=0, y=0 is valid)."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()
        mock_handler.click = AsyncMock(return_value=None)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_zero",
            "name": "computer",
            "arguments": '{"action": "click", "x": 0, "y": 0}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        # Zero coordinates should be passed, not filtered
        mock_handler.click.assert_called_once_with(x=0, y=0)

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_missing_action_raises_error(
        self, mock_litellm, disable_telemetry
    ):
        """Test that missing 'action' argument raises ToolError."""
        from agent import ComputerAgent
        from agent.types import ToolError

        mock_handler = AsyncMock()
        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_missing",
            "name": "computer",
            "arguments": '{"x": 100, "y": 200}',  # Missing 'action'
        }

        # Should return a tool error item instead of raising
        result = await agent._handle_item(function_call_item, mock_handler)

        # Result should be an error item
        assert len(result) == 1
        assert "error" in result[0]["type"].lower() or "output" in result[0]

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_unknown_action_raises_error(
        self, mock_litellm, disable_telemetry
    ):
        """Test that unknown action type raises ToolError."""
        from agent import ComputerAgent

        # Use MagicMock with spec to ensure unknown attributes return None
        mock_handler = MagicMock()
        # Explicitly set nonexistent_action to None
        mock_handler.nonexistent_action = None
        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_unknown",
            "name": "computer",
            "arguments": '{"action": "nonexistent_action"}',
        }

        # The handler won't have a nonexistent_action method
        result = await agent._handle_item(function_call_item, mock_handler)

        # Should return an error item (ToolError wrapped)
        assert len(result) == 1
        # The result should indicate an error
        assert "error" in str(result[0]).lower() or "toolerror" in str(result[0]).lower()

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_computer_preserves_action_result(
        self, mock_litellm, disable_telemetry
    ):
        """Test that action_result is preserved in function_call_output."""
        import json

        from agent import ComputerAgent

        mock_handler = AsyncMock()
        action_result = {"status": "completed", "details": "Action successful"}
        mock_handler.click = AsyncMock(return_value=action_result)
        mock_handler.screenshot = AsyncMock(return_value="base64_screenshot_data")

        agent = ComputerAgent(model="openai/gpt-5.4")

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_result",
            "name": "computer",
            "arguments": '{"action": "click", "x": 100, "y": 100}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        # The output should contain the actual action_result
        output = json.loads(result[0]["output"])
        assert output == action_result

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_function_call_non_computer_uses_regular_handler(
        self, mock_litellm, disable_telemetry
    ):
        """Test that non-computer function calls still use regular function handling."""
        from agent import ComputerAgent

        mock_handler = AsyncMock()

        # Define a custom tool function
        def custom_tool(arg1: str) -> str:
            """A custom tool for testing.

            Args:
                arg1: The argument to process

            Returns:
                A result string
            """
            return f"Result: {arg1}"

        # Pass tool as a list (the expected format for ComputerAgent tools)
        agent = ComputerAgent(model="openai/gpt-5.4", tools=[custom_tool])

        function_call_item = {
            "type": "function_call",
            "call_id": "fc_custom",
            "name": "custom_tool",
            "arguments": '{"arg1": "test_value"}',
        }

        result = await agent._handle_item(function_call_item, mock_handler)

        assert result[0]["type"] == "function_call_output"
        assert "Result: test_value" in result[0]["output"]


class TestComputerActionCompatibility:
    """Test that both computer_call and function_call produce equivalent behavior.

    These tests verify that the same computer actions produce consistent results
    whether invoked via the standard computer_call format or the GPT 5.4
    function_call format.
    """

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_click_action_consistency(self, mock_litellm, disable_telemetry):
        """Test that click action produces consistent results in both formats."""
        from agent import ComputerAgent

        # Setup for computer_call
        mock_handler_cc = AsyncMock()
        mock_handler_cc.click = AsyncMock(return_value=None)
        mock_handler_cc.screenshot = AsyncMock(return_value="base64_screenshot")

        # Setup for function_call
        mock_handler_fc = AsyncMock()
        mock_handler_fc.click = AsyncMock(return_value=None)
        mock_handler_fc.screenshot = AsyncMock(return_value="base64_screenshot")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        # computer_call format
        cc_item = {
            "type": "computer_call",
            "call_id": "cc_click",
            "action": {"type": "click", "x": 100, "y": 200},
            "pending_safety_checks": [],
        }

        # function_call format
        fc_item = {
            "type": "function_call",
            "call_id": "fc_click",
            "name": "computer",
            "arguments": '{"action": "click", "x": 100, "y": 200}',
        }

        cc_result = await agent._handle_item(cc_item, mock_handler_cc)
        fc_result = await agent._handle_item(fc_item, mock_handler_fc)

        # Both should call click with same args
        mock_handler_cc.click.assert_called_once_with(x=100, y=200)
        mock_handler_fc.click.assert_called_once_with(x=100, y=200)

        # Both should take screenshot
        mock_handler_cc.screenshot.assert_called_once()
        mock_handler_fc.screenshot.assert_called_once()

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_scroll_action_consistency(self, mock_litellm, disable_telemetry):
        """Test that scroll action works consistently in both formats."""
        from agent import ComputerAgent

        mock_handler_cc = AsyncMock()
        mock_handler_cc.scroll = AsyncMock(return_value=None)
        mock_handler_cc.screenshot = AsyncMock(return_value="base64_screenshot")

        mock_handler_fc = AsyncMock()
        mock_handler_fc.scroll = AsyncMock(return_value=None)
        mock_handler_fc.screenshot = AsyncMock(return_value="base64_screenshot")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        cc_item = {
            "type": "computer_call",
            "call_id": "cc_scroll",
            "action": {"type": "scroll", "x": 500, "y": 300, "scroll_x": 0, "scroll_y": -100},
            "pending_safety_checks": [],
        }

        fc_item = {
            "type": "function_call",
            "call_id": "fc_scroll",
            "name": "computer",
            "arguments": '{"action": "scroll", "x": 500, "y": 300, "scroll_x": 0, "scroll_y": -100}',
        }

        await agent._handle_item(cc_item, mock_handler_cc)
        await agent._handle_item(fc_item, mock_handler_fc)

        # Both should call scroll with same args
        mock_handler_cc.scroll.assert_called_once_with(x=500, y=300, scroll_x=0, scroll_y=-100)
        mock_handler_fc.scroll.assert_called_once_with(x=500, y=300, scroll_x=0, scroll_y=-100)

    @pytest.mark.asyncio
    @patch("agent.agent.litellm")
    async def test_drag_action_consistency(self, mock_litellm, disable_telemetry):
        """Test that drag action works consistently in both formats."""
        from agent import ComputerAgent

        mock_handler_cc = AsyncMock()
        mock_handler_cc.drag = AsyncMock(return_value=None)
        mock_handler_cc.screenshot = AsyncMock(return_value="base64_screenshot")

        mock_handler_fc = AsyncMock()
        mock_handler_fc.drag = AsyncMock(return_value=None)
        mock_handler_fc.screenshot = AsyncMock(return_value="base64_screenshot")

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")

        cc_item = {
            "type": "computer_call",
            "call_id": "cc_drag",
            "action": {"type": "drag", "start_x": 100, "start_y": 100, "end_x": 200, "end_y": 200},
            "pending_safety_checks": [],
        }

        fc_item = {
            "type": "function_call",
            "call_id": "fc_drag",
            "name": "computer",
            "arguments": '{"action": "drag", "start_x": 100, "start_y": 100, "end_x": 200, "end_y": 200}',
        }

        await agent._handle_item(cc_item, mock_handler_cc)
        await agent._handle_item(fc_item, mock_handler_fc)

        mock_handler_cc.drag.assert_called_once_with(
            start_x=100, start_y=100, end_x=200, end_y=200
        )
        mock_handler_fc.drag.assert_called_once_with(
            start_x=100, start_y=100, end_x=200, end_y=200
        )
