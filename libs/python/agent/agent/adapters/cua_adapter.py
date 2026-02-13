import json
import os
from typing import Any, AsyncIterator, Iterator, Optional

import httpx
from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse


class CUAAdapter(CustomLLM):
    """
    CUA Adapter for routing requests through the CUA Inference API.

    Makes direct HTTP calls to the inference API endpoints:
    - /v1/messages (Anthropic format)
    - /v1/chat/completions (OpenAI format)
    - /v1/gemini/* (Gemini pass-through)

    For streaming, uses direct HTTP with SSE parsing instead of nested liteLLM calls.
    """

    def __init__(self, base_url: str | None = None, api_key: str | None = None, **_: Any):
        super().__init__()
        self.base_url = base_url or os.environ.get("CUA_BASE_URL") or "https://inference.cua.ai/v1"
        self.api_key = (
            api_key or os.environ.get("CUA_INFERENCE_API_KEY") or os.environ.get("CUA_API_KEY")
        )
        self._async_client: Optional[httpx.AsyncClient] = None
        self._sync_client: Optional[httpx.Client] = None

    def _get_async_client(self) -> httpx.AsyncClient:
        """Get or create async HTTP client."""
        if self._async_client is None:
            self._async_client = httpx.AsyncClient(timeout=600.0)
        return self._async_client

    def _get_sync_client(self) -> httpx.Client:
        """Get or create sync HTTP client."""
        if self._sync_client is None:
            self._sync_client = httpx.Client(timeout=600.0)
        return self._sync_client

    def _normalize_model(self, model: str) -> str:
        """Remove cua/ prefix if present, then remove provider prefix."""
        # First strip cua/ if present
        if model.startswith("cua/"):
            model = model[4:]
        # Return remaining model string (may still have provider prefix like anthropic/)
        return model

    def _get_anthropic_model_name(self, model: str) -> str:
        """
        Get the model name for Anthropic API.

        The inference API expects 'anthropic/claude-sonnet-4.5' format for validation.
        Input could be: cua/anthropic/claude-sonnet-4.5 or anthropic/claude-sonnet-4.5
        """
        normalized = self._normalize_model(model)
        # Ensure anthropic/ prefix is present
        if not normalized.startswith("anthropic/"):
            normalized = f"anthropic/{normalized}"
        return normalized

    def completion(self, *args, **kwargs) -> ModelResponse:
        model = kwargs.get("model", "")
        api_base = kwargs.get("api_base") or self.base_url
        if "anthropic/" in model:
            model = f"anthropic/{self._normalize_model(model)}"
            api_base = api_base.removesuffix("/v1")
        elif "gemini/" in model or "google/" in model:
            model = f"gemini/{self._normalize_model(model)}"
            api_base = api_base + "/gemini"
        else:
            model = f"openai/{self._normalize_model(model)}"

        params = {
            "model": model,
            "messages": kwargs.get("messages", []),
            "api_base": api_base,
            "api_key": kwargs.get("api_key") or self.api_key,
            "stream": False,
        }

        # Forward tools if provided
        if "tools" in kwargs:
            params["tools"] = kwargs["tools"]

        if "optional_params" in kwargs:
            params.update(kwargs["optional_params"])

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]

        return completion(**params)  # type: ignore

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        model = kwargs.get("model", "")
        api_base = kwargs.get("api_base") or self.base_url
        if "anthropic/" in model:
            model = f"anthropic/{self._normalize_model(model)}"
            api_base = api_base.removesuffix("/v1")
        elif "gemini/" in model or "google/" in model:
            model = f"gemini/{self._normalize_model(model)}"
            api_base = api_base + "/gemini"
        else:
            model = f"openai/{self._normalize_model(model)}"

        params = {
            "model": model,
            "messages": kwargs.get("messages", []),
            "api_base": api_base,
            "api_key": kwargs.get("api_key") or self.api_key,
            "stream": False,
        }

        # Forward tools if provided
        if "tools" in kwargs:
            params["tools"] = kwargs["tools"]

        if "optional_params" in kwargs:
            params.update(kwargs["optional_params"])

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]

        response = await acompletion(**params)  # type: ignore
        return response

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        """
        Synchronous streaming using direct HTTP calls.

        Currently only supports Anthropic models.
        """
        model = kwargs.get("model", "")
        api_key = kwargs.get("api_key") or self.api_key

        if "anthropic/" in model:
            yield from self._stream_anthropic_sync(model, kwargs, api_key)
        else:
            raise NotImplementedError(
                f"Streaming is only supported for Anthropic models. Got: {model}"
            )

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        """
        Asynchronous streaming using direct HTTP calls.

        Currently only supports Anthropic models.
        """
        model = kwargs.get("model", "")
        api_key = kwargs.get("api_key") or self.api_key

        if "anthropic/" in model:
            async for chunk in self._stream_anthropic_async(model, kwargs, api_key):
                yield chunk
        else:
            raise NotImplementedError(
                f"Streaming is only supported for Anthropic models. Got: {model}"
            )

    def _stream_anthropic_sync(
        self, model: str, kwargs: dict, api_key: str
    ) -> Iterator[GenericStreamingChunk]:
        """Stream from Anthropic-compatible /v1/messages endpoint."""
        api_base = kwargs.get("api_base") or self.base_url
        # Remove /v1 suffix for Anthropic endpoint
        api_base = api_base.removesuffix("/v1")
        url = f"{api_base.rstrip('/')}/v1/messages"

        headers = {
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "anthropic-version": "2023-06-01",
        }

        # Add beta header if tools are provided (for computer use)
        if "tools" in kwargs or kwargs.get("optional_params", {}).get("tools"):
            headers["anthropic-beta"] = "computer-use-2025-01-24"

        # Get any extra headers
        if "headers" in kwargs:
            headers.update(kwargs["headers"])

        body = {
            "model": self._get_anthropic_model_name(model),
            "messages": kwargs.get("messages", []),
            "max_tokens": kwargs.get("max_tokens", 4096),
            "stream": True,
        }

        if "tools" in kwargs:
            body["tools"] = kwargs["tools"]
        if "system" in kwargs:
            body["system"] = kwargs["system"]

        client = self._get_sync_client()

        with client.stream("POST", url, headers=headers, json=body) as response:
            response.raise_for_status()

            current_event = None
            for line in response.iter_lines():
                if line.startswith("event: "):
                    current_event = line[7:]
                elif line.startswith("data: "):
                    data = line[6:]
                    try:
                        chunk_json = json.loads(data)
                        chunk = self._parse_anthropic_sse_event(current_event, chunk_json)
                        if chunk:
                            yield chunk
                            # Check if this is the final chunk - if so, break to allow clean exit
                            if chunk.get("is_finished"):
                                break
                    except json.JSONDecodeError:
                        continue

    async def _stream_anthropic_async(
        self, model: str, kwargs: dict, api_key: str
    ) -> AsyncIterator[GenericStreamingChunk]:
        """Stream from Anthropic-compatible /v1/messages endpoint."""
        api_base = kwargs.get("api_base") or self.base_url
        # Remove /v1 suffix for Anthropic endpoint
        api_base = api_base.removesuffix("/v1")
        url = f"{api_base.rstrip('/')}/v1/messages"

        headers = {
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "anthropic-version": "2023-06-01",
        }

        # Add beta header if tools are provided (for computer use)
        if "tools" in kwargs or kwargs.get("optional_params", {}).get("tools"):
            headers["anthropic-beta"] = "computer-use-2025-01-24"

        # Get any extra headers
        if "headers" in kwargs:
            headers.update(kwargs["headers"])

        body = {
            "model": self._get_anthropic_model_name(model),
            "messages": kwargs.get("messages", []),
            "max_tokens": kwargs.get("max_tokens", 4096),
            "stream": True,
        }

        if "tools" in kwargs:
            body["tools"] = kwargs["tools"]
        if "system" in kwargs:
            body["system"] = kwargs["system"]

        client = self._get_async_client()

        async with client.stream("POST", url, headers=headers, json=body) as response:
            response.raise_for_status()

            current_event = None
            async for line in response.aiter_lines():
                if line.startswith("event: "):
                    current_event = line[7:]
                elif line.startswith("data: "):
                    data = line[6:]
                    try:
                        chunk_json = json.loads(data)
                        chunk = self._parse_anthropic_sse_event(current_event, chunk_json)
                        if chunk:
                            yield chunk
                            # Check if this is the final chunk - if so, break to allow clean exit
                            if chunk.get("is_finished"):
                                break
                    except json.JSONDecodeError:
                        continue

    def _parse_anthropic_sse_event(
        self, event_type: Optional[str], data: dict
    ) -> Optional[GenericStreamingChunk]:
        """
        Parse Anthropic SSE event into GenericStreamingChunk.

        Anthropic SSE events:
        - message_start: Contains initial message metadata
        - content_block_start: Start of a content block
        - content_block_delta: Incremental text content
        - content_block_stop: End of a content block
        - message_delta: Final message metadata with usage
        - message_stop: End of message
        """
        if event_type == "content_block_delta":
            delta = data.get("delta", {})
            text = delta.get("text", "")
            if text:
                return {
                    "finish_reason": None,
                    "index": 0,
                    "is_finished": False,
                    "text": text,
                    "tool_use": None,
                    "usage": None,
                }

        elif event_type == "message_delta":
            # Contains usage info and stop_reason
            delta = data.get("delta", {})
            stop_reason = delta.get("stop_reason")
            usage = data.get("usage", {})

            return {
                "finish_reason": stop_reason,
                "index": 0,
                "is_finished": stop_reason is not None,
                "text": "",
                "tool_use": None,
                "usage": {
                    "completion_tokens": usage.get("output_tokens", 0),
                    "prompt_tokens": 0,  # Not available in message_delta
                    "total_tokens": usage.get("output_tokens", 0),
                },
            }

        elif event_type == "message_stop":
            return {
                "finish_reason": "stop",
                "index": 0,
                "is_finished": True,
                "text": "",
                "tool_use": None,
                "usage": None,
            }

        # For other events (message_start, content_block_start, content_block_stop),
        # we don't need to yield anything
        return None

    def __del__(self):
        """Cleanup HTTP clients."""
        try:
            if self._sync_client is not None:
                self._sync_client.close()
        except Exception:
            pass

        try:
            if self._async_client is not None:
                import asyncio

                try:
                    loop = asyncio.get_event_loop()
                    if loop.is_running():
                        loop.create_task(self._async_client.aclose())
                    else:
                        loop.run_until_complete(self._async_client.aclose())
                except Exception:
                    pass
        except (ImportError, RuntimeError):
            # Python interpreter is shutting down
            pass
