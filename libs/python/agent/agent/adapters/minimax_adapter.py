"""
MiniMax adapter for litellm - routes minimax/ prefixed models to the MiniMax API.

MiniMax provides OpenAI-compatible API endpoints at https://api.minimax.io/v1.
Supported models: MiniMax-M2.5, MiniMax-M2.5-highspeed (204K context window).

Environment variable: MINIMAX_API_KEY
"""

import os
from typing import Any, AsyncIterator, Iterator

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse

MINIMAX_API_BASE = "https://api.minimax.io/v1"


class MiniMaxAdapter(CustomLLM):
    def __init__(self, base_url: str | None = None, api_key: str | None = None, **_: Any):
        super().__init__()
        self.base_url = base_url or os.environ.get("MINIMAX_API_BASE") or MINIMAX_API_BASE
        self.api_key = api_key or os.environ.get("MINIMAX_API_KEY")

    def _normalize_model(self, model: str) -> str:
        """Strip the minimax/ prefix to get the bare model name."""
        if model.startswith("minimax/"):
            return model[len("minimax/"):]
        return model

    def _resolve_api_key(self, kwargs: dict | None = None) -> str:
        """Resolve the MiniMax API key, raising a clear error if missing."""
        resolved = (kwargs.get("api_key") if kwargs else None) or self.api_key
        if not resolved:
            raise ValueError(
                "No MiniMax API key provided. "
                "Please either set the MINIMAX_API_KEY environment variable "
                "or pass api_key to ComputerAgent()."
            )
        return resolved

    def _clamp_temperature(self, kwargs: dict) -> None:
        """Clamp temperature to MiniMax's accepted range [0, 1.0]."""
        if "temperature" in kwargs:
            temp = kwargs["temperature"]
            if temp is not None:
                kwargs["temperature"] = max(0.0, min(float(temp), 1.0))

    def _build_params(self, kwargs: dict, stream: bool = False) -> dict:
        """Build parameters for the inner litellm call."""
        model = self._normalize_model(kwargs.get("model", ""))
        api_key = self._resolve_api_key(kwargs)

        self._clamp_temperature(kwargs)

        extra_headers = {}
        if "extra_headers" in kwargs:
            extra_headers.update(kwargs.pop("extra_headers"))
        extra_headers["Authorization"] = f"Bearer {api_key}"

        params = {
            "model": f"openai/{model}",
            "messages": kwargs.get("messages", []),
            "api_base": self.base_url,
            "api_key": api_key,
            "extra_headers": extra_headers,
            "stream": stream,
        }

        # Forward tools if provided
        if "tools" in kwargs:
            params["tools"] = kwargs["tools"]
        if "tool_choice" in kwargs:
            params["tool_choice"] = kwargs["tool_choice"]

        # Forward optional generation params
        for key in ("temperature", "top_p", "max_completion_tokens", "max_tokens", "response_format"):
            if key in kwargs:
                params[key] = kwargs[key]

        if "optional_params" in kwargs:
            protected_keys = {"api_key", "extra_headers", "model", "api_base", "stream"}
            filtered = {
                k: v for k, v in kwargs["optional_params"].items() if k not in protected_keys
            }
            params.update(filtered)

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]

        return params

    def completion(self, *args, **kwargs) -> ModelResponse:
        params = self._build_params(kwargs)
        return completion(**params)  # type: ignore

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        params = self._build_params(kwargs)
        response = await acompletion(**params)  # type: ignore
        return response

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        params = self._build_params(kwargs, stream=True)
        for chunk in completion(**params):  # type: ignore
            yield chunk  # type: ignore

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        params = self._build_params(kwargs, stream=True)
        stream = await acompletion(**params)  # type: ignore
        async for chunk in stream:  # type: ignore
            yield chunk  # type: ignore
