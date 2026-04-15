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


class TestComputerAgentStructuredOutputs:
    """Test ComputerAgent structured outputs support."""

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_output_type(self, mock_litellm, disable_telemetry):
        """Test that agent can be initialized with output_type parameter."""
        from pydantic import BaseModel

        from agent import ComputerAgent

        class MyOutput(BaseModel):
            title: str
            score: int

        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929",
            output_type=MyOutput,
        )

        assert agent is not None
        assert agent.output_type is MyOutput

    @patch("agent.agent.litellm")
    def test_agent_initialization_without_output_type(self, mock_litellm, disable_telemetry):
        """Test that output_type defaults to None."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")
        assert agent.output_type is None

    def test_pydantic_model_to_response_format(self):
        """Test conversion of Pydantic model to response_format dict."""
        from pydantic import BaseModel

        from agent.agent import _pydantic_model_to_response_format

        class TestModel(BaseModel):
            name: str
            value: int

        result = _pydantic_model_to_response_format(TestModel)

        assert result["type"] == "json_schema"
        assert result["json_schema"]["name"] == "TestModel"
        assert "properties" in result["json_schema"]["schema"]
        assert "name" in result["json_schema"]["schema"]["properties"]
        assert "value" in result["json_schema"]["schema"]["properties"]

    def test_extract_text_from_output_item_dict(self):
        """Test extracting text from a dict-based output item."""
        from agent.agent import _extract_text_from_output_item

        item = {
            "role": "assistant",
            "type": "message",
            "content": [
                {"type": "output_text", "text": '{"title": "test", "score": 42}'}
            ],
        }

        result = _extract_text_from_output_item(item)
        assert result == '{"title": "test", "score": 42}'

    def test_extract_text_from_output_item_no_text(self):
        """Test extracting text from an item with no text content."""
        from agent.agent import _extract_text_from_output_item

        item = {"role": "assistant", "type": "message", "content": []}
        result = _extract_text_from_output_item(item)
        assert result is None


class TestComputerAgentIntegration:
    """Test ComputerAgent integration with Computer tool (SRP: Integration within package)."""

    def test_agent_accepts_computer_tool(self, disable_telemetry, mock_computer):
        """Test that agent can be initialized with Computer tool."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", tools=[mock_computer])

        # Verify agent accepted the tool
        assert agent is not None
        assert hasattr(agent, "tools")
