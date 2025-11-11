import os
from typing import Any, AsyncIterator, Iterator

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse


class CUAAdapter(CustomLLM):
    def __init__(self, base_url: str | None = None, api_key: str | None = None, **_: Any):
        super().__init__()
        self.base_url = base_url or os.environ.get("CUA_BASE_URL") or "https://inference.cua.ai/v1"
        self.api_key = api_key or os.environ.get("CUA_INFERENCE_API_KEY") or os.environ.get("CUA_API_KEY")

    def _normalize_model(self, model: str) -> str:
        # Accept either "cua/<model>" or raw "<model>"
        return model.split("/", 1)[1] if model and model.startswith("cua/") else model

    def completion(self, *args, **kwargs) -> ModelResponse:
        params = dict(kwargs)
        inner_model = self._normalize_model(params.get("model", ""))
        params.update(
            {
                "model": f"openai/{inner_model}",
                "api_base": self.base_url,
                "api_key": self.api_key,
                "stream": False,
            }
        )
        return completion(**params)  # type: ignore

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        params = dict(kwargs)
        inner_model = self._normalize_model(params.get("model", ""))
        params.update(
            {
                "model": f"openai/{inner_model}",
                "api_base": self.base_url,
                "api_key": self.api_key,
                "stream": False,
            }
        )
        return await acompletion(**params)  # type: ignore

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        params = dict(kwargs)
        inner_model = self._normalize_model(params.get("model", ""))
        params.update(
            {
                "model": f"openai/{inner_model}",
                "api_base": self.base_url,
                "api_key": self.api_key,
                "stream": True,
            }
        )
        # Yield chunks directly from LiteLLM's streaming generator
        for chunk in completion(**params):  # type: ignore
            yield chunk  # type: ignore

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        params = dict(kwargs)
        inner_model = self._normalize_model(params.get("model", ""))
        params.update(
            {
                "model": f"openai/{inner_model}",
                "api_base": self.base_url,
                "api_key": self.api_key,
                "stream": True,
            }
        )
        stream = await acompletion(**params)  # type: ignore
        async for chunk in stream:  # type: ignore
            yield chunk  # type: ignore
