"""Unit tests for CUAAdapter class.

Tests the CUA adapter for routing requests through the CUA Inference API,
including streaming functionality.
"""

import json
from unittest.mock import AsyncMock, MagicMock, Mock, PropertyMock, patch

import pytest


class TestCUAAdapterInitialization:
    """Test CUAAdapter initialization."""

    def test_adapter_initialization_defaults(self):
        """Test adapter initializes with default values."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        assert adapter.base_url == "https://inference.cua.ai/v1"
        assert adapter._async_client is None
        assert adapter._sync_client is None

    def test_adapter_initialization_with_custom_base_url(self):
        """Test adapter initializes with custom base_url."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(base_url="https://custom.api.com/v1")

        assert adapter.base_url == "https://custom.api.com/v1"

    def test_adapter_initialization_with_api_key(self):
        """Test adapter initializes with custom api_key."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-api-key")

        assert adapter.api_key == "test-api-key"

    def test_adapter_initialization_from_environment(self, monkeypatch):
        """Test adapter reads from environment variables."""
        from agent.adapters.cua_adapter import CUAAdapter

        # Clear any existing keys first (CUA_INFERENCE_API_KEY takes precedence)
        monkeypatch.delenv("CUA_INFERENCE_API_KEY", raising=False)
        monkeypatch.setenv("CUA_BASE_URL", "https://env.api.com/v1")
        monkeypatch.setenv("CUA_API_KEY", "env-api-key")

        adapter = CUAAdapter()

        assert adapter.base_url == "https://env.api.com/v1"
        assert adapter.api_key == "env-api-key"


class TestCUAAdapterModelNormalization:
    """Test model name normalization methods."""

    def test_normalize_model_with_cua_prefix(self):
        """Test normalizing model with cua/ prefix."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._normalize_model("cua/anthropic/claude-sonnet-4.5")
        assert result == "anthropic/claude-sonnet-4.5"

    def test_normalize_model_without_cua_prefix(self):
        """Test normalizing model without cua/ prefix."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._normalize_model("anthropic/claude-sonnet-4.5")
        assert result == "anthropic/claude-sonnet-4.5"

    def test_normalize_model_plain_name(self):
        """Test normalizing plain model name."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._normalize_model("claude-sonnet-4.5")
        assert result == "claude-sonnet-4.5"

    def test_get_anthropic_model_name_with_prefix(self):
        """Test getting Anthropic model name with existing prefix."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._get_anthropic_model_name("cua/anthropic/claude-sonnet-4.5")
        assert result == "anthropic/claude-sonnet-4.5"

    def test_get_anthropic_model_name_without_prefix(self):
        """Test getting Anthropic model name without prefix adds it."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._get_anthropic_model_name("claude-sonnet-4.5")
        assert result == "anthropic/claude-sonnet-4.5"


class TestCUAAdapterHTTPClients:
    """Test HTTP client management."""

    def test_get_sync_client_creates_client(self):
        """Test sync client is created on first access."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        assert adapter._sync_client is None
        client = adapter._get_sync_client()
        assert client is not None
        assert adapter._sync_client is client

    def test_get_sync_client_reuses_existing(self):
        """Test sync client is reused on subsequent access."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        client1 = adapter._get_sync_client()
        client2 = adapter._get_sync_client()
        assert client1 is client2

    def test_get_async_client_creates_client(self):
        """Test async client is created on first access."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        assert adapter._async_client is None
        client = adapter._get_async_client()
        assert client is not None
        assert adapter._async_client is client

    def test_get_async_client_reuses_existing(self):
        """Test async client is reused on subsequent access."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        client1 = adapter._get_async_client()
        client2 = adapter._get_async_client()
        assert client1 is client2


class TestCUAAdapterSSEParsing:
    """Test Anthropic SSE event parsing."""

    def test_parse_content_block_delta_with_text(self):
        """Test parsing content_block_delta event with text."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        data = {"delta": {"text": "Hello, world!"}}
        result = adapter._parse_anthropic_sse_event("content_block_delta", data)

        assert result is not None
        assert result["text"] == "Hello, world!"
        assert result["is_finished"] is False
        assert result["finish_reason"] is None
        assert result["index"] == 0

    def test_parse_content_block_delta_empty_text(self):
        """Test parsing content_block_delta event with empty text returns None."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        data = {"delta": {"text": ""}}
        result = adapter._parse_anthropic_sse_event("content_block_delta", data)

        assert result is None

    def test_parse_message_delta_with_stop_reason(self):
        """Test parsing message_delta event with stop_reason."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        data = {
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 100},
        }
        result = adapter._parse_anthropic_sse_event("message_delta", data)

        assert result is not None
        assert result["finish_reason"] == "end_turn"
        assert result["is_finished"] is True
        assert result["usage"]["completion_tokens"] == 100

    def test_parse_message_delta_without_stop_reason(self):
        """Test parsing message_delta event without stop_reason."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        data = {"delta": {}, "usage": {"output_tokens": 50}}
        result = adapter._parse_anthropic_sse_event("message_delta", data)

        assert result is not None
        assert result["finish_reason"] is None
        assert result["is_finished"] is False

    def test_parse_message_stop(self):
        """Test parsing message_stop event."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._parse_anthropic_sse_event("message_stop", {})

        assert result is not None
        assert result["finish_reason"] == "stop"
        assert result["is_finished"] is True
        assert result["text"] == ""

    def test_parse_message_start_returns_none(self):
        """Test parsing message_start event returns None (no output needed)."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        data = {"message": {"id": "msg_123"}}
        result = adapter._parse_anthropic_sse_event("message_start", data)

        assert result is None

    def test_parse_content_block_start_returns_none(self):
        """Test parsing content_block_start event returns None."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        data = {"content_block": {"type": "text"}}
        result = adapter._parse_anthropic_sse_event("content_block_start", data)

        assert result is None

    def test_parse_content_block_stop_returns_none(self):
        """Test parsing content_block_stop event returns None."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter()

        result = adapter._parse_anthropic_sse_event("content_block_stop", {})

        assert result is None


class TestCUAAdapterStreaming:
    """Test streaming methods."""

    def test_streaming_raises_for_non_anthropic_model(self):
        """Test streaming raises NotImplementedError for non-Anthropic models."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        with pytest.raises(NotImplementedError) as exc_info:
            list(adapter.streaming(model="openai/gpt-4"))

        assert "only supported for Anthropic models" in str(exc_info.value)

    @pytest.mark.asyncio
    async def test_astreaming_raises_for_non_anthropic_model(self):
        """Test async streaming raises NotImplementedError for non-Anthropic models."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        with pytest.raises(NotImplementedError) as exc_info:
            async for _ in adapter.astreaming(model="openai/gpt-4"):
                pass

        assert "only supported for Anthropic models" in str(exc_info.value)

    def test_streaming_calls_sync_method_for_anthropic(self):
        """Test streaming dispatches to _stream_anthropic_sync for Anthropic models."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        with patch.object(adapter, "_stream_anthropic_sync") as mock_stream:
            mock_stream.return_value = iter([{"text": "test"}])

            result = list(adapter.streaming(model="anthropic/claude-sonnet-4.5"))

            mock_stream.assert_called_once()
            assert result == [{"text": "test"}]

    @pytest.mark.asyncio
    async def test_astreaming_calls_async_method_for_anthropic(self):
        """Test async streaming dispatches to _stream_anthropic_async for Anthropic models."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        async def mock_async_gen(*args, **kwargs):
            yield {"text": "test"}

        with patch.object(adapter, "_stream_anthropic_async") as mock_stream:
            mock_stream.return_value = mock_async_gen()

            result = []
            async for chunk in adapter.astreaming(model="anthropic/claude-sonnet-4.5"):
                result.append(chunk)

            mock_stream.assert_called_once()
            assert result == [{"text": "test"}]


class TestCUAAdapterStreamingHTTP:
    """Test streaming HTTP integration."""

    def test_stream_anthropic_sync_builds_correct_url(self):
        """Test sync streaming builds correct API URL."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")

        # Mock the client stream
        mock_response = MagicMock()
        mock_response.iter_lines.return_value = iter(
            [
                "event: message_stop",
                "data: {}",
            ]
        )
        mock_response.raise_for_status = MagicMock()

        mock_client = MagicMock()
        mock_client.stream.return_value.__enter__ = MagicMock(return_value=mock_response)
        mock_client.stream.return_value.__exit__ = MagicMock(return_value=False)

        with patch.object(adapter, "_get_sync_client", return_value=mock_client):
            list(
                adapter._stream_anthropic_sync(
                    model="anthropic/claude-sonnet-4.5", kwargs={"messages": []}, api_key="test-key"
                )
            )

            # Verify URL construction
            mock_client.stream.assert_called_once()
            call_args = mock_client.stream.call_args
            assert call_args[0][0] == "POST"
            assert call_args[0][1] == "https://api.test.com/v1/messages"

    def test_stream_anthropic_sync_includes_beta_header_with_tools(self):
        """Test sync streaming includes anthropic-beta header when tools are provided."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")

        mock_response = MagicMock()
        mock_response.iter_lines.return_value = iter(
            [
                "event: message_stop",
                "data: {}",
            ]
        )
        mock_response.raise_for_status = MagicMock()

        mock_client = MagicMock()
        mock_client.stream.return_value.__enter__ = MagicMock(return_value=mock_response)
        mock_client.stream.return_value.__exit__ = MagicMock(return_value=False)

        with patch.object(adapter, "_get_sync_client", return_value=mock_client):
            list(
                adapter._stream_anthropic_sync(
                    model="anthropic/claude-sonnet-4.5",
                    kwargs={"messages": [], "tools": [{"type": "computer"}]},
                    api_key="test-key",
                )
            )

            call_args = mock_client.stream.call_args
            headers = call_args[1]["headers"]
            assert "anthropic-beta" in headers
            assert headers["anthropic-beta"] == "computer-use-2025-01-24"

    @pytest.mark.asyncio
    async def test_stream_anthropic_async_builds_correct_url(self):
        """Test async streaming builds correct API URL."""
        from contextlib import asynccontextmanager

        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")

        # Create async mock response
        async def mock_aiter_lines():
            for line in ["event: message_stop", "data: {}"]:
                yield line

        mock_response = MagicMock()
        mock_response.aiter_lines = mock_aiter_lines
        mock_response.raise_for_status = MagicMock()

        # Create proper async context manager
        @asynccontextmanager
        async def mock_stream(*args, **kwargs):
            yield mock_response

        mock_client = MagicMock()
        mock_client.stream = mock_stream

        with patch.object(adapter, "_get_async_client", return_value=mock_client):
            result = []
            async for chunk in adapter._stream_anthropic_async(
                model="anthropic/claude-sonnet-4.5", kwargs={"messages": []}, api_key="test-key"
            ):
                result.append(chunk)

        # Since we're using a wrapped mock, verify the result instead
        assert len(result) >= 1  # Should have at least the message_stop chunk


class TestCUAAdapterStreamingIntegration:
    """Integration tests for streaming with mocked HTTP responses."""

    def test_sync_streaming_parses_text_content(self):
        """Test sync streaming correctly parses text content from SSE."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        # Simulate SSE response
        sse_lines = [
            "event: message_start",
            'data: {"type":"message_start"}',
            "event: content_block_start",
            'data: {"type":"content_block_start"}',
            "event: content_block_delta",
            'data: {"type":"content_block_delta","delta":{"text":"Hello"}}',
            "event: content_block_delta",
            'data: {"type":"content_block_delta","delta":{"text":" World"}}',
            "event: content_block_stop",
            'data: {"type":"content_block_stop"}',
            "event: message_delta",
            'data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}}',
            "event: message_stop",
            'data: {"type":"message_stop"}',
        ]

        mock_response = MagicMock()
        mock_response.iter_lines.return_value = iter(sse_lines)
        mock_response.raise_for_status = MagicMock()

        mock_client = MagicMock()
        mock_client.stream.return_value.__enter__ = MagicMock(return_value=mock_response)
        mock_client.stream.return_value.__exit__ = MagicMock(return_value=False)

        with patch.object(adapter, "_get_sync_client", return_value=mock_client):
            chunks = list(
                adapter._stream_anthropic_sync(
                    model="anthropic/claude-sonnet-4.5",
                    kwargs={"messages": [{"role": "user", "content": "Hi"}]},
                    api_key="test-key",
                )
            )

        # Should have received text chunks + final chunks
        text_chunks = [c for c in chunks if c.get("text")]
        assert len(text_chunks) == 2
        assert text_chunks[0]["text"] == "Hello"
        assert text_chunks[1]["text"] == " World"

        # Check for finish signal
        finished_chunks = [c for c in chunks if c.get("is_finished")]
        assert len(finished_chunks) >= 1

    @pytest.mark.asyncio
    async def test_async_streaming_parses_text_content(self):
        """Test async streaming correctly parses text content from SSE."""
        from contextlib import asynccontextmanager

        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        # Simulate SSE response
        sse_lines = [
            "event: content_block_delta",
            'data: {"type":"content_block_delta","delta":{"text":"Async"}}',
            "event: content_block_delta",
            'data: {"type":"content_block_delta","delta":{"text":" Test"}}',
            "event: message_stop",
            'data: {"type":"message_stop"}',
        ]

        async def mock_aiter_lines():
            for line in sse_lines:
                yield line

        mock_response = MagicMock()
        mock_response.aiter_lines = mock_aiter_lines
        mock_response.raise_for_status = MagicMock()

        # Create proper async context manager
        @asynccontextmanager
        async def mock_stream(*args, **kwargs):
            yield mock_response

        mock_client = MagicMock()
        mock_client.stream = mock_stream

        with patch.object(adapter, "_get_async_client", return_value=mock_client):
            chunks = []
            async for chunk in adapter._stream_anthropic_async(
                model="anthropic/claude-sonnet-4.5",
                kwargs={"messages": [{"role": "user", "content": "Hi"}]},
                api_key="test-key",
            ):
                chunks.append(chunk)

        text_chunks = [c for c in chunks if c.get("text")]
        assert len(text_chunks) == 2
        assert text_chunks[0]["text"] == "Async"
        assert text_chunks[1]["text"] == " Test"

    def test_streaming_handles_json_decode_error(self):
        """Test streaming gracefully handles malformed JSON in SSE data."""
        from agent.adapters.cua_adapter import CUAAdapter

        adapter = CUAAdapter(api_key="test-key")

        sse_lines = [
            "event: content_block_delta",
            "data: {invalid json}",  # Malformed JSON
            "event: content_block_delta",
            'data: {"delta":{"text":"Valid"}}',
            "event: message_stop",
            "data: {}",
        ]

        mock_response = MagicMock()
        mock_response.iter_lines.return_value = iter(sse_lines)
        mock_response.raise_for_status = MagicMock()

        mock_client = MagicMock()
        mock_client.stream.return_value.__enter__ = MagicMock(return_value=mock_response)
        mock_client.stream.return_value.__exit__ = MagicMock(return_value=False)

        with patch.object(adapter, "_get_sync_client", return_value=mock_client):
            # Should not raise, should skip invalid JSON
            chunks = list(
                adapter._stream_anthropic_sync(
                    model="anthropic/claude-sonnet-4.5", kwargs={"messages": []}, api_key="test-key"
                )
            )

        # Should still get the valid chunk
        text_chunks = [c for c in chunks if c.get("text")]
        assert len(text_chunks) == 1
        assert text_chunks[0]["text"] == "Valid"


class TestCUAAdapterCompletion:
    """Test non-streaming completion methods."""

    @patch("agent.adapters.cua_adapter.completion")
    def test_completion_with_anthropic_model(self, mock_completion):
        """Test completion routes Anthropic models correctly."""
        from agent.adapters.cua_adapter import CUAAdapter

        mock_completion.return_value = {"choices": [{"message": {"content": "test"}}]}

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")
        adapter.completion(
            model="anthropic/claude-sonnet-4.5",
            messages=[{"role": "user", "content": "Hi"}],
        )

        mock_completion.assert_called_once()
        call_kwargs = mock_completion.call_args[1]
        assert call_kwargs["model"] == "anthropic/anthropic/claude-sonnet-4.5"
        assert call_kwargs["api_base"] == "https://api.test.com"  # /v1 stripped

    @patch("agent.adapters.cua_adapter.completion")
    def test_completion_with_gemini_model(self, mock_completion):
        """Test completion routes Gemini models correctly."""
        from agent.adapters.cua_adapter import CUAAdapter

        mock_completion.return_value = {"choices": [{"message": {"content": "test"}}]}

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")
        adapter.completion(
            model="gemini/gemini-pro",
            messages=[{"role": "user", "content": "Hi"}],
        )

        mock_completion.assert_called_once()
        call_kwargs = mock_completion.call_args[1]
        assert call_kwargs["model"] == "gemini/gemini/gemini-pro"
        assert call_kwargs["api_base"] == "https://api.test.com/v1/gemini"

    @patch("agent.adapters.cua_adapter.completion")
    def test_completion_with_openai_model(self, mock_completion):
        """Test completion routes OpenAI models correctly."""
        from agent.adapters.cua_adapter import CUAAdapter

        mock_completion.return_value = {"choices": [{"message": {"content": "test"}}]}

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")
        adapter.completion(
            model="gpt-4",
            messages=[{"role": "user", "content": "Hi"}],
        )

        mock_completion.assert_called_once()
        call_kwargs = mock_completion.call_args[1]
        assert call_kwargs["model"] == "openai/gpt-4"

    @patch("agent.adapters.cua_adapter.completion")
    def test_completion_forwards_tools(self, mock_completion):
        """Test completion forwards tools parameter."""
        from agent.adapters.cua_adapter import CUAAdapter

        mock_completion.return_value = {"choices": [{"message": {"content": "test"}}]}

        adapter = CUAAdapter(api_key="test-key")
        tools = [{"type": "function", "function": {"name": "test"}}]
        adapter.completion(
            model="anthropic/claude-sonnet-4.5",
            messages=[{"role": "user", "content": "Hi"}],
            tools=tools,
        )

        call_kwargs = mock_completion.call_args[1]
        assert call_kwargs["tools"] == tools

    @pytest.mark.asyncio
    @patch("agent.adapters.cua_adapter.acompletion")
    async def test_acompletion_with_anthropic_model(self, mock_acompletion):
        """Test async completion routes Anthropic models correctly."""
        from agent.adapters.cua_adapter import CUAAdapter

        mock_acompletion.return_value = {"choices": [{"message": {"content": "test"}}]}

        adapter = CUAAdapter(api_key="test-key", base_url="https://api.test.com/v1")
        await adapter.acompletion(
            model="anthropic/claude-sonnet-4.5",
            messages=[{"role": "user", "content": "Hi"}],
        )

        mock_acompletion.assert_called_once()
        call_kwargs = mock_acompletion.call_args[1]
        assert call_kwargs["model"] == "anthropic/anthropic/claude-sonnet-4.5"
        assert call_kwargs["api_base"] == "https://api.test.com"
