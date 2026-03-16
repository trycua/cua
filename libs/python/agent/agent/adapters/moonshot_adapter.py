"""
Moonshot adapter for litellm - routes moonshot/ prefixed models to the Moonshot API.
"""

import os
from typing import Any, AsyncIterator, Iterator

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse

MOONSHOT_API_BASE = "https://api.moonshot.ai/v1"


class MoonshotAdapter(CustomLLM):
    def __init__(self, base_url: str | None = None, api_key: str | None = None, **_: Any):
        super().__init__()
        self.base_url = base_url or os.environ.get("MOONSHOT_API_BASE") or MOONSHOT_API_BASE
        self.api_key = api_key or os.environ.get("MOONSHOT_API_KEY")

    def _normalize_model(self, model: str) -> str:
        """Strip the moonshot/ prefix to get the bare model name."""
        if model.startswith("moonshot/"):
            return model[len("moonshot/"):]
        return model

    def _resolve_api_key(self, kwargs: dict | None = None) -> str:
        """Resolve the Moonshot API key, raising a clear error if missing."""
        resolved = (kwargs.get("api_key") if kwargs else None) or self.api_key
        if not resolved:
            raise ValueError(
                "No Moonshot API key provided. "
                "Please either set the MOONSHOT_API_KEY environment variable "
                "or pass api_key to ComputerAgent()."
            )
        return resolved

    def _build_params(self, kwargs: dict) -> dict:
        """Build parameters for the inner litellm call."""
        model = self._normalize_model(kwargs.get("model", ""))
        api_key = self._resolve_api_key(kwargs)

        params = {
            "model": f"openai/{model}",
            "messages": kwargs.get("messages", []),
            "api_base": self.base_url,
            "api_key": api_key,
        }

        if "stream" in kwargs:
            params["stream"] = kwargs["stream"]

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
            protected_keys = {"api_key", "model", "api_base", "stream"}
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
        params = self._build_params(kwargs)
        params["stream"] = True
        return completion(**params)  # type: ignore

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        params = self._build_params(kwargs)
        params["stream"] = True
        return await acompletion(**params)  # type: ignore
