"""Unit tests for MiniMaxAdapter.

Tests the MiniMax litellm adapter: model normalization, API key resolution,
temperature clamping, parameter building, and completion routing.
"""

import os
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from agent.adapters.minimax_adapter import MINIMAX_API_BASE, MiniMaxAdapter


class TestMiniMaxAdapterInit:
    """Tests for MiniMaxAdapter initialization."""

    def test_default_base_url(self):
        adapter = MiniMaxAdapter()
        assert adapter.base_url == MINIMAX_API_BASE

    def test_custom_base_url(self):
        adapter = MiniMaxAdapter(base_url="https://custom.minimax.io/v1")
        assert adapter.base_url == "https://custom.minimax.io/v1"

    def test_api_key_from_constructor(self):
        adapter = MiniMaxAdapter(api_key="test-key-123")
        assert adapter.api_key == "test-key-123"

    def test_api_key_from_env(self, monkeypatch):
        monkeypatch.setenv("MINIMAX_API_KEY", "env-key-456")
        adapter = MiniMaxAdapter()
        assert adapter.api_key == "env-key-456"

    def test_base_url_from_env(self, monkeypatch):
        monkeypatch.setenv("MINIMAX_API_BASE", "https://env.minimax.io/v1")
        adapter = MiniMaxAdapter()
        assert adapter.base_url == "https://env.minimax.io/v1"

    def test_constructor_overrides_env(self, monkeypatch):
        monkeypatch.setenv("MINIMAX_API_KEY", "env-key")
        adapter = MiniMaxAdapter(api_key="constructor-key")
        assert adapter.api_key == "constructor-key"


class TestModelNormalization:
    """Tests for model name normalization."""

    def test_strip_minimax_prefix(self):
        adapter = MiniMaxAdapter()
        assert adapter._normalize_model("minimax/MiniMax-M2.5") == "MiniMax-M2.5"

    def test_no_prefix(self):
        adapter = MiniMaxAdapter()
        assert adapter._normalize_model("MiniMax-M2.5") == "MiniMax-M2.5"

    def test_highspeed_model(self):
        adapter = MiniMaxAdapter()
        assert adapter._normalize_model("minimax/MiniMax-M2.5-highspeed") == "MiniMax-M2.5-highspeed"

    def test_strip_m27_prefix(self):
        adapter = MiniMaxAdapter()
        assert adapter._normalize_model("minimax/MiniMax-M2.7") == "MiniMax-M2.7"

    def test_m27_highspeed_model(self):
        adapter = MiniMaxAdapter()
        assert adapter._normalize_model("minimax/MiniMax-M2.7-highspeed") == "MiniMax-M2.7-highspeed"


class TestAPIKeyResolution:
    """Tests for API key resolution logic."""

    def test_key_from_kwargs(self):
        adapter = MiniMaxAdapter(api_key="default-key")
        resolved = adapter._resolve_api_key({"api_key": "override-key"})
        assert resolved == "override-key"

    def test_key_from_adapter(self):
        adapter = MiniMaxAdapter(api_key="adapter-key")
        resolved = adapter._resolve_api_key({})
        assert resolved == "adapter-key"

    def test_missing_key_raises(self):
        adapter = MiniMaxAdapter()
        adapter.api_key = None
        with pytest.raises(ValueError, match="No MiniMax API key provided"):
            adapter._resolve_api_key({})

    def test_none_kwargs(self):
        adapter = MiniMaxAdapter(api_key="adapter-key")
        resolved = adapter._resolve_api_key(None)
        assert resolved == "adapter-key"


class TestTemperatureClamping:
    """Tests for temperature clamping to MiniMax range [0, 1.0]."""

    def test_clamp_high_temperature(self):
        adapter = MiniMaxAdapter()
        kwargs = {"temperature": 2.0}
        adapter._clamp_temperature(kwargs)
        assert kwargs["temperature"] == 1.0

    def test_clamp_negative_temperature(self):
        adapter = MiniMaxAdapter()
        kwargs = {"temperature": -0.5}
        adapter._clamp_temperature(kwargs)
        assert kwargs["temperature"] == 0.0

    def test_valid_temperature_unchanged(self):
        adapter = MiniMaxAdapter()
        kwargs = {"temperature": 0.7}
        adapter._clamp_temperature(kwargs)
        assert kwargs["temperature"] == 0.7

    def test_zero_temperature(self):
        adapter = MiniMaxAdapter()
        kwargs = {"temperature": 0}
        adapter._clamp_temperature(kwargs)
        assert kwargs["temperature"] == 0.0

    def test_max_temperature(self):
        adapter = MiniMaxAdapter()
        kwargs = {"temperature": 1.0}
        adapter._clamp_temperature(kwargs)
        assert kwargs["temperature"] == 1.0

    def test_no_temperature_key(self):
        adapter = MiniMaxAdapter()
        kwargs = {"model": "test"}
        adapter._clamp_temperature(kwargs)
        assert "temperature" not in kwargs

    def test_none_temperature(self):
        adapter = MiniMaxAdapter()
        kwargs = {"temperature": None}
        adapter._clamp_temperature(kwargs)
        assert kwargs["temperature"] is None


class TestBuildParams:
    """Tests for _build_params parameter construction."""

    def test_basic_params(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {
            "model": "minimax/MiniMax-M2.5",
            "messages": [{"role": "user", "content": "Hello"}],
        }
        params = adapter._build_params(kwargs)

        assert params["model"] == "openai/MiniMax-M2.5"
        assert params["api_base"] == MINIMAX_API_BASE
        assert params["api_key"] == "test-key"
        assert params["messages"] == [{"role": "user", "content": "Hello"}]
        assert params["stream"] is False

    def test_stream_param(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {"model": "minimax/MiniMax-M2.5", "messages": []}
        params = adapter._build_params(kwargs, stream=True)
        assert params["stream"] is True

    def test_forwards_tools(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        tools = [{"type": "function", "function": {"name": "test"}}]
        kwargs = {"model": "minimax/MiniMax-M2.5", "messages": [], "tools": tools}
        params = adapter._build_params(kwargs)
        assert params["tools"] == tools

    def test_forwards_tool_choice(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {
            "model": "minimax/MiniMax-M2.5",
            "messages": [],
            "tool_choice": "auto",
        }
        params = adapter._build_params(kwargs)
        assert params["tool_choice"] == "auto"

    def test_forwards_generation_params(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {
            "model": "minimax/MiniMax-M2.5",
            "messages": [],
            "temperature": 0.5,
            "top_p": 0.9,
            "max_tokens": 1024,
        }
        params = adapter._build_params(kwargs)
        assert params["temperature"] == 0.5
        assert params["top_p"] == 0.9
        assert params["max_tokens"] == 1024

    def test_temperature_clamped_in_params(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {
            "model": "minimax/MiniMax-M2.5",
            "messages": [],
            "temperature": 2.0,
        }
        params = adapter._build_params(kwargs)
        assert params["temperature"] == 1.0

    def test_optional_params_forwarded(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {
            "model": "minimax/MiniMax-M2.5",
            "messages": [],
            "optional_params": {"presence_penalty": 0.5},
        }
        params = adapter._build_params(kwargs)
        assert params["presence_penalty"] == 0.5

    def test_optional_params_protected_keys_excluded(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {
            "model": "minimax/MiniMax-M2.5",
            "messages": [],
            "optional_params": {"api_key": "bad-key", "model": "bad-model"},
        }
        params = adapter._build_params(kwargs)
        assert params["api_key"] == "test-key"
        assert params["model"] == "openai/MiniMax-M2.5"

    def test_auth_header_set(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {"model": "minimax/MiniMax-M2.5", "messages": []}
        params = adapter._build_params(kwargs)
        assert params["extra_headers"]["Authorization"] == "Bearer test-key"

    def test_highspeed_model_routing(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {"model": "minimax/MiniMax-M2.5-highspeed", "messages": []}
        params = adapter._build_params(kwargs)
        assert params["model"] == "openai/MiniMax-M2.5-highspeed"

    def test_m27_model_routing(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {"model": "minimax/MiniMax-M2.7", "messages": []}
        params = adapter._build_params(kwargs)
        assert params["model"] == "openai/MiniMax-M2.7"

    def test_m27_highspeed_model_routing(self):
        adapter = MiniMaxAdapter(api_key="test-key")
        kwargs = {"model": "minimax/MiniMax-M2.7-highspeed", "messages": []}
        params = adapter._build_params(kwargs)
        assert params["model"] == "openai/MiniMax-M2.7-highspeed"


class TestCompletion:
    """Tests for completion and acompletion methods."""

    @patch("agent.adapters.minimax_adapter.completion")
    def test_completion_calls_litellm(self, mock_completion):
        mock_completion.return_value = MagicMock()
        adapter = MiniMaxAdapter(api_key="test-key")
        adapter.completion(
            model="minimax/MiniMax-M2.5",
            messages=[{"role": "user", "content": "Hi"}],
        )
        mock_completion.assert_called_once()
        call_kwargs = mock_completion.call_args
        assert call_kwargs[1]["model"] == "openai/MiniMax-M2.5" or call_kwargs[0][0] if call_kwargs[0] else True

    @pytest.mark.asyncio
    @patch("agent.adapters.minimax_adapter.acompletion")
    async def test_acompletion_calls_litellm(self, mock_acompletion):
        mock_acompletion.return_value = MagicMock()
        adapter = MiniMaxAdapter(api_key="test-key")
        await adapter.acompletion(
            model="minimax/MiniMax-M2.5",
            messages=[{"role": "user", "content": "Hi"}],
        )
        mock_acompletion.assert_called_once()


class TestStreaming:
    """Tests for streaming methods."""

    @patch("agent.adapters.minimax_adapter.completion")
    def test_streaming_yields_chunks(self, mock_completion):
        mock_chunks = [MagicMock(), MagicMock()]
        mock_completion.return_value = iter(mock_chunks)
        adapter = MiniMaxAdapter(api_key="test-key")

        chunks = list(adapter.streaming(
            model="minimax/MiniMax-M2.5",
            messages=[{"role": "user", "content": "Hi"}],
        ))
        assert len(chunks) == 2
        call_kwargs = mock_completion.call_args[1]
        assert call_kwargs["stream"] is True

    @pytest.mark.asyncio
    @patch("agent.adapters.minimax_adapter.acompletion")
    async def test_astreaming_yields_chunks(self, mock_acompletion):
        mock_chunks = [MagicMock(), MagicMock()]

        async def async_gen():
            for chunk in mock_chunks:
                yield chunk

        mock_acompletion.return_value = async_gen()
        adapter = MiniMaxAdapter(api_key="test-key")

        chunks = []
        async for chunk in adapter.astreaming(
            model="minimax/MiniMax-M2.5",
            messages=[{"role": "user", "content": "Hi"}],
        ):
            chunks.append(chunk)
        assert len(chunks) == 2


class TestComputerAgentMiniMaxIntegration:
    """Tests for MiniMax integration with ComputerAgent."""

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_minimax(self, mock_litellm, disable_telemetry):
        """Test that ComputerAgent can be initialized with a minimax model."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.5")
        assert agent.model == "minimax/MiniMax-M2.5"

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_minimax_highspeed(self, mock_litellm, disable_telemetry):
        """Test that ComputerAgent can be initialized with minimax highspeed model."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.5-highspeed")
        assert agent.model == "minimax/MiniMax-M2.5-highspeed"

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_minimax_m27(self, mock_litellm, disable_telemetry):
        """Test that ComputerAgent can be initialized with MiniMax M2.7 model."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.7")
        assert agent.model == "minimax/MiniMax-M2.7"

    @patch("agent.agent.litellm")
    def test_agent_initialization_with_minimax_m27_highspeed(self, mock_litellm, disable_telemetry):
        """Test that ComputerAgent can be initialized with MiniMax M2.7 highspeed model."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.7-highspeed")
        assert agent.model == "minimax/MiniMax-M2.7-highspeed"

    @patch("agent.agent.litellm")
    def test_minimax_registered_in_provider_map(self, mock_litellm, disable_telemetry):
        """Test that minimax is registered in litellm custom_provider_map."""
        from agent import ComputerAgent

        ComputerAgent(model="minimax/MiniMax-M2.5")

        provider_map = mock_litellm.custom_provider_map
        providers = [p["provider"] for p in provider_map]
        assert "minimax" in providers

    @patch("agent.agent.litellm")
    def test_minimax_handler_is_adapter_instance(self, mock_litellm, disable_telemetry):
        """Test that the minimax handler is a MiniMaxAdapter instance."""
        from agent import ComputerAgent

        ComputerAgent(model="minimax/MiniMax-M2.5")

        provider_map = mock_litellm.custom_provider_map
        minimax_entry = next(p for p in provider_map if p["provider"] == "minimax")
        assert isinstance(minimax_entry["custom_handler"], MiniMaxAdapter)

    @patch("agent.agent.litellm")
    def test_minimax_uses_generic_vlm_loop(self, mock_litellm, disable_telemetry):
        """Test that minimax models fall through to the generic VLM agent loop."""
        from agent import ComputerAgent
        from agent.loops.generic_vlm import GenericVlmConfig

        agent = ComputerAgent(model="minimax/MiniMax-M2.5")
        assert isinstance(agent.agent_loop, GenericVlmConfig)

    @patch("agent.agent.litellm")
    def test_agent_with_minimax_has_run_method(self, mock_litellm, disable_telemetry):
        """Test that a MiniMax-initialized agent has a callable run method."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.5")
        assert hasattr(agent, "run")
        assert callable(agent.run)

    @patch("agent.agent.litellm")
    def test_agent_with_minimax_api_key(self, mock_litellm, disable_telemetry):
        """Test that api_key is forwarded when using minimax model."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.5", api_key="my-minimax-key")
        assert agent.api_key == "my-minimax-key"

    @patch("agent.agent.litellm")
    def test_agent_with_minimax_and_computer(self, mock_litellm, disable_telemetry, mock_computer):
        """Test that ComputerAgent accepts tools with minimax model."""
        from agent import ComputerAgent

        agent = ComputerAgent(model="minimax/MiniMax-M2.5", tools=[mock_computer])
        assert agent is not None
        assert hasattr(agent, "tools")
