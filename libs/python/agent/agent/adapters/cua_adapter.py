import os
from typing import Any, AsyncIterator, Iterator

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse


class CUAAdapter(CustomLLM):
    def __init__(self, base_url: str | None = None, api_key: str | None = None, **_: Any):
        super().__init__()
        self.base_url = base_url or os.environ.get("CUA_BASE_URL") or "https://inference.cua.ai/v1"
        self.api_key = (
            api_key or os.environ.get("CUA_INFERENCE_API_KEY") or os.environ.get("CUA_API_KEY")
        )

    def _normalize_model(self, model: str) -> str:
        # Accept either "cua/<model>" or raw "<model>"
        return model.split("/", 1)[1] if model and model.startswith("cua/") else model

    def completion(self, *args, **kwargs) -> ModelResponse:
        model = kwargs.get("model", "")
        api_base = kwargs.get("api_base") or self.base_url
        if "anthropic/" in model:
            model = f"anthropic/{self._normalize_model(model)}"
            api_base = api_base.removesuffix("/v1")
        elif "gemini/" in model or "google/" in model:
            # Route to Gemini pass-through endpoint
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
            del kwargs["optional_params"]

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]
            del kwargs["headers"]

        # Print dropped parameters
        original_keys = set(kwargs.keys())
        used_keys = set(params.keys())  # Only these are extracted from kwargs
        ignored_keys = {
            "litellm_params",
            "client",
            "print_verbose",
            "acompletion",
            "timeout",
            "logging_obj",
            "encoding",
            "custom_prompt_dict",
            "model_response",
            "logger_fn",
        }
        dropped_keys = original_keys - used_keys - ignored_keys
        if dropped_keys:
            dropped_keyvals = {k: kwargs[k] for k in dropped_keys}
            # print(f"CUAAdapter.completion: Dropped parameters: {dropped_keyvals}")

        return completion(**params)  # type: ignore

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        model = kwargs.get("model", "")
        api_base = kwargs.get("api_base") or self.base_url
        if "anthropic/" in model:
            model = f"anthropic/{self._normalize_model(model)}"
            api_base = api_base.removesuffix("/v1")
        elif "gemini/" in model or "google/" in model:
            # Route to Gemini pass-through endpoint
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
            del kwargs["optional_params"]

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]
            del kwargs["headers"]

        # Print dropped parameters
        original_keys = set(kwargs.keys())
        used_keys = set(params.keys())  # Only these are extracted from kwargs
        ignored_keys = {
            "litellm_params",
            "client",
            "print_verbose",
            "acompletion",
            "timeout",
            "logging_obj",
            "encoding",
            "custom_prompt_dict",
            "model_response",
            "logger_fn",
        }
        dropped_keys = original_keys - used_keys - ignored_keys
        if dropped_keys:
            dropped_keyvals = {k: kwargs[k] for k in dropped_keys}
            # print(f"CUAAdapter.acompletion: Dropped parameters: {dropped_keyvals}")

        response = await acompletion(**params)  # type: ignore

        return response

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
