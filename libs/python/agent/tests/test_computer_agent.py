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

        agent = ComputerAgent(model="anthropic/claude-3-5-sonnet-20241022")

        assert agent is not None
        assert hasattr(agent, "model")
        assert agent.model == "anthropic/claude-3-5-sonnet-20241022"

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_tools(self, mock_litellm, disable_telemetry, mock_computer):
        """Test that agent can be initialized with tools."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-3-5-sonnet-20241022", tools=[mock_computer])

        assert agent is not None
        assert hasattr(agent, "tools")

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_max_budget(self, mock_litellm, disable_telemetry):
        """Test that agent can be initialized with max trajectory budget."""
        from agent import ComputerAgent

        budget = 5.0
        agent = ComputerAgent(
            model="anthropic/claude-3-5-sonnet-20241022", max_trajectory_budget=budget
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

        agent = ComputerAgent(model="anthropic/claude-3-5-sonnet-20241022")

        # Run should return an async generator
        result_generator = agent.run(sample_messages)

        assert result_generator is not None
        # Check it's an async generator
        assert hasattr(result_generator, "__anext__")

    def test_agent_has_run_method(self, disable_telemetry):
        """Test that agent has run method available."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-3-5-sonnet-20241022")

        # Verify run method exists
        assert hasattr(agent, "run")
        assert callable(agent.run)

    def test_agent_has_agent_loop(self, disable_telemetry):
        """Test that agent has agent_loop initialized."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="anthropic/claude-3-5-sonnet-20241022")

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

        agent = ComputerAgent(model="anthropic/claude-3-5-sonnet-20241022", tools=[mock_computer])

        # Verify agent accepted the tool
        assert agent is not None
        assert hasattr(agent, "tools")
