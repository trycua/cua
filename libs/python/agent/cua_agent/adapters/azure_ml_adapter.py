"""
Azure ML Custom Provider Adapter for LiteLLM.

This adapter provides direct OpenAI-compatible API access to Azure ML endpoints
without message transformation, specifically for models like Fara-7B that require
exact OpenAI message formatting.
"""

import json
from typing import Any, AsyncIterator, Dict, Iterator, List, Optional

import httpx
from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse


class AzureMLAdapter(CustomLLM):
    """
    Azure ML Adapter for OpenAI-compatible endpoints.

    Makes direct HTTP calls to Azure ML foundry inference endpoints
    using the OpenAI-compatible API format without transforming messages.

    Usage:
        model = "azure_ml/Fara-7B"
        api_base = "https://foundry-inference-xxx.centralus.inference.ml.azure.com"
        api_key = "your-api-key"

        response = litellm.completion(
            model=model,
            messages=[...],
            api_base=api_base,
            api_key=api_key
        )
    """

    def __init__(self, **kwargs):
        """Initialize the adapter."""
        super().__init__()
        self._client: Optional[httpx.Client] = None
        self._async_client: Optional[httpx.AsyncClient] = None

    def _get_client(self) -> httpx.Client:
        """Get or create sync HTTP client."""
        if self._client is None:
            self._client = httpx.Client(timeout=600.0)
        return self._client

    def _get_async_client(self) -> httpx.AsyncClient:
        """Get or create async HTTP client."""
        if self._async_client is None:
            self._async_client = httpx.AsyncClient(timeout=600.0)
        return self._async_client

    def _prepare_request(self, **kwargs) -> tuple[str, dict, dict]:
        """
        Prepare the HTTP request without transforming messages.

        Applies Azure ML workaround: double-encodes function arguments to work around
        Azure ML's bug where it parses arguments before validation.

        Returns:
            Tuple of (url, headers, json_data)
        """
        # Extract required params
        api_base = kwargs.get("api_base")
        api_key = kwargs.get("api_key")
        model = kwargs.get("model", "").replace("azure_ml/", "")
        messages = kwargs.get("messages", [])

        if not api_base:
            raise ValueError("api_base is required for azure_ml provider")
        if not api_key:
            raise ValueError("api_key is required for azure_ml provider")

        # Build OpenAI-compatible endpoint URL
        base_url = api_base.rstrip("/")
        url = f"{base_url}/chat/completions"

        # Prepare headers
        headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}

        # WORKAROUND for Azure ML bug:
        # Azure ML incorrectly parses the arguments field before validation,
        # causing it to reject valid JSON strings. We double-encode arguments
        # so that after Azure ML's parse, they remain as strings.
        messages_copy = []
        for message in messages:
            msg_copy = message.copy()

            # Check if message has tool_calls that need double-encoding
            if "tool_calls" in msg_copy:
                tool_calls_copy = []
                for tool_call in msg_copy["tool_calls"]:
                    tc_copy = tool_call.copy()

                    if "function" in tc_copy and "arguments" in tc_copy["function"]:
                        func_copy = tc_copy["function"].copy()
                        arguments = func_copy["arguments"]

                        # If arguments is already a string, double-encode it
                        if isinstance(arguments, str):
                            func_copy["arguments"] = json.dumps(arguments)

                        tc_copy["function"] = func_copy

                    tool_calls_copy.append(tc_copy)

                msg_copy["tool_calls"] = tool_calls_copy

            messages_copy.append(msg_copy)

        # Prepare request body with double-encoded messages
        json_data = {"model": model, "messages": messages_copy}

        # Add optional parameters if provided
        optional_params = [
            "temperature",
            "top_p",
            "n",
            "stream",
            "stop",
            "max_tokens",
            "presence_penalty",
            "frequency_penalty",
            "logit_bias",
            "user",
            "response_format",
            "seed",
            "tools",
            "tool_choice",
        ]

        for param in optional_params:
            if param in kwargs and kwargs[param] is not None:
                json_data[param] = kwargs[param]

        return url, headers, json_data

    def completion(self, *args, **kwargs) -> ModelResponse:
        """
        Synchronous completion method.

        Makes a direct HTTP POST to Azure ML's OpenAI-compatible endpoint.
        """
        url, headers, json_data = self._prepare_request(**kwargs)

        client = self._get_client()
        response = client.post(url, headers=headers, json=json_data)
        response.raise_for_status()

        # Parse response
        response_json = response.json()

        # Return using litellm's completion with the actual response
        return completion(
            model=f"azure_ml/{kwargs.get('model', '')}",
            mock_response=response_json["choices"][0]["message"]["content"],
            messages=kwargs.get("messages", []),
        )

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        """
        Asynchronous completion method.

        Makes a direct async HTTP POST to Azure ML's OpenAI-compatible endpoint.
        """
        url, headers, json_data = self._prepare_request(**kwargs)

        client = self._get_async_client()
        response = await client.post(url, headers=headers, json=json_data)
        response.raise_for_status()

        # Parse response
        response_json = response.json()

        # Return using litellm's acompletion with the actual response
        return await acompletion(
            model=f"azure_ml/{kwargs.get('model', '')}",
            mock_response=response_json["choices"][0]["message"]["content"],
            messages=kwargs.get("messages", []),
        )

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        """
        Synchronous streaming method.

        Makes a streaming HTTP POST to Azure ML's OpenAI-compatible endpoint.
        """
        url, headers, json_data = self._prepare_request(**kwargs)
        json_data["stream"] = True

        client = self._get_client()

        with client.stream("POST", url, headers=headers, json=json_data) as response:
            response.raise_for_status()

            for line in response.iter_lines():
                if line.startswith("data: "):
                    data = line[6:]  # Remove "data: " prefix
                    if data == "[DONE]":
                        break

                    try:
                        chunk_json = json.loads(data)
                        delta = chunk_json["choices"][0].get("delta", {})
                        content = delta.get("content", "")
                        finish_reason = chunk_json["choices"][0].get("finish_reason")

                        generic_streaming_chunk: GenericStreamingChunk = {
                            "finish_reason": finish_reason,
                            "index": 0,
                            "is_finished": finish_reason is not None,
                            "text": content,
                            "tool_use": None,
                            "usage": chunk_json.get(
                                "usage",
                                {"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0},
                            ),
                        }

                        yield generic_streaming_chunk
                    except json.JSONDecodeError:
                        continue

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        """
        Asynchronous streaming method.

        Makes an async streaming HTTP POST to Azure ML's OpenAI-compatible endpoint.
        """
        url, headers, json_data = self._prepare_request(**kwargs)
        json_data["stream"] = True

        client = self._get_async_client()

        async with client.stream("POST", url, headers=headers, json=json_data) as response:
            response.raise_for_status()

            async for line in response.aiter_lines():
                if line.startswith("data: "):
                    data = line[6:]  # Remove "data: " prefix
                    if data == "[DONE]":
                        break

                    try:
                        chunk_json = json.loads(data)
                        delta = chunk_json["choices"][0].get("delta", {})
                        content = delta.get("content", "")
                        finish_reason = chunk_json["choices"][0].get("finish_reason")

                        generic_streaming_chunk: GenericStreamingChunk = {
                            "finish_reason": finish_reason,
                            "index": 0,
                            "is_finished": finish_reason is not None,
                            "text": content,
                            "tool_use": None,
                            "usage": chunk_json.get(
                                "usage",
                                {"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0},
                            ),
                        }

                        yield generic_streaming_chunk
                    except json.JSONDecodeError:
                        continue

    def __del__(self):
        """Cleanup HTTP clients."""
        if self._client is not None:
            self._client.close()
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
