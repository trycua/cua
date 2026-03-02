import os
from typing import Any, AsyncIterator, Iterator

from core.http import cua_version_headers
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
        """Strip known prefixes to get the base model name."""
        known_prefixes = ("cua/", "anthropic/", "gemini/", "google/", "openai/")
        result = model
        for prefix in known_prefixes:
            if result.startswith(prefix):
                result = result[len(prefix) :]
        return result

    def _resolve_route(self, model: str, api_base: str) -> tuple[str, str]:
        """Return (prefixed_model, api_base) for the CUA inference API."""
        if "anthropic/" in model:
            return f"anthropic/{self._normalize_model(model)}", api_base.removesuffix("/v1")
        elif "gemini/" in model or "google/" in model:
            return f"gemini/{self._normalize_model(model)}", api_base + "/gemini"
        else:
            return f"openai/{self._normalize_model(model)}", api_base

    def _resolve_api_key(self, kwargs: dict | None = None) -> str:
        """Resolve the CUA API key, raising a clear error if missing.

        Checks kwargs (from ComputerAgent api_key param) then falls back
        to self.api_key (from CUA_API_KEY / CUA_INFERENCE_API_KEY env vars).

        This validation must run before the inner litellm call because that
        call uses an anthropic/ or openai/ model prefix, which would cause
        litellm to fall back to ANTHROPIC_API_KEY from env — sending the
        wrong key to the CUA inference endpoint.
        """
        resolved = (kwargs.get("api_key") if kwargs else None) or self.api_key
        if not resolved:
            raise ValueError(
                "No CUA API key provided for cua/ model inference. "
                "Please either set the CUA_API_KEY environment variable "
                "or pass api_key to ComputerAgent()."
            )
        return resolved

    def completion(self, *args, **kwargs) -> ModelResponse:
        model, api_base = self._resolve_route(
            kwargs.get("model", ""), kwargs.get("api_base") or self.base_url
        )

        api_key = self._resolve_api_key(kwargs)

        # Ensure the CUA inference API always receives Bearer auth;
        # merge caller headers first, then force Authorization so it cannot be overridden.
        extra_headers = {}
        if "extra_headers" in kwargs:
            extra_headers.update(kwargs.pop("extra_headers"))
        extra_headers["Authorization"] = f"Bearer {api_key}"

        params = {
            "model": model,
            "messages": kwargs.get("messages", []),
            "api_base": api_base,
            "api_key": api_key,
            "extra_headers": extra_headers,
            "stream": False,
        }

        # Forward tools if provided
        if "tools" in kwargs:
            params["tools"] = kwargs["tools"]

        if "optional_params" in kwargs:
            protected_keys = {"api_key", "extra_headers", "model", "api_base", "stream"}
            filtered = {
                k: v for k, v in kwargs["optional_params"].items() if k not in protected_keys
            }
            params.update(filtered)
            del kwargs["optional_params"]

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]
            del kwargs["headers"]

        # Always include CUA version headers
        version_hdrs = cua_version_headers()
        if version_hdrs:
            params["headers"] = {**version_hdrs, **params.get("headers", {})}

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
        model, api_base = self._resolve_route(
            kwargs.get("model", ""), kwargs.get("api_base") or self.base_url
        )

        api_key = self._resolve_api_key(kwargs)

        # Ensure the CUA inference API always receives Bearer auth;
        # merge caller headers first, then force Authorization so it cannot be overridden.
        extra_headers = {}
        if "extra_headers" in kwargs:
            extra_headers.update(kwargs.pop("extra_headers"))
        extra_headers["Authorization"] = f"Bearer {api_key}"

        params = {
            "model": model,
            "messages": kwargs.get("messages", []),
            "api_base": api_base,
            "api_key": api_key,
            "extra_headers": extra_headers,
            "stream": False,
        }

        # Forward tools if provided
        if "tools" in kwargs:
            params["tools"] = kwargs["tools"]

        if "optional_params" in kwargs:
            protected_keys = {"api_key", "extra_headers", "model", "api_base", "stream"}
            filtered = {
                k: v for k, v in kwargs["optional_params"].items() if k not in protected_keys
            }
            params.update(filtered)
            del kwargs["optional_params"]

        if "headers" in kwargs:
            params["headers"] = kwargs["headers"]
            del kwargs["headers"]

        # Always include CUA version headers
        version_hdrs = cua_version_headers()
        if version_hdrs:
            params["headers"] = {**version_hdrs, **params.get("headers", {})}

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
        model, api_base = self._resolve_route(
            params.get("model", ""), params.get("api_base") or self.base_url
        )
        api_key = self._resolve_api_key(kwargs)

        # Ensure the CUA inference API always receives Bearer auth;
        # merge caller headers first, then force Authorization so it cannot be overridden.
        extra_headers = {}
        if "extra_headers" in params:
            extra_headers.update(params.pop("extra_headers"))
        extra_headers["Authorization"] = f"Bearer {api_key}"

        params.update(
            {
                "model": model,
                "api_base": api_base,
                "api_key": api_key,
                "extra_headers": extra_headers,
                "stream": True,
            }
        )
        # Always include CUA version headers
        version_hdrs = cua_version_headers()
        if version_hdrs:
            params["headers"] = {**version_hdrs, **params.get("headers", {})}
        # Yield chunks directly from LiteLLM's streaming generator
        for chunk in completion(**params):  # type: ignore
            yield chunk  # type: ignore

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        params = dict(kwargs)
        model, api_base = self._resolve_route(
            params.get("model", ""), params.get("api_base") or self.base_url
        )
        api_key = self._resolve_api_key(kwargs)

        # Ensure the CUA inference API always receives Bearer auth;
        # merge caller headers first, then force Authorization so it cannot be overridden.
        extra_headers = {}
        if "extra_headers" in params:
            extra_headers.update(params.pop("extra_headers"))
        extra_headers["Authorization"] = f"Bearer {api_key}"

        params.update(
            {
                "model": model,
                "api_base": api_base,
                "api_key": api_key,
                "extra_headers": extra_headers,
                "stream": True,
            }
        )
        # Always include CUA version headers
        version_hdrs = cua_version_headers()
        if version_hdrs:
            params["headers"] = {**version_hdrs, **params.get("headers", {})}
        stream = await acompletion(**params)  # type: ignore
        async for chunk in stream:  # type: ignore
            yield chunk  # type: ignore
