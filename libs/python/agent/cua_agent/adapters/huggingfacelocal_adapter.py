import asyncio
import functools
import warnings
from concurrent.futures import ThreadPoolExecutor
from typing import Any, AsyncIterator, Dict, Iterator, List, Optional

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse

# Try to import HuggingFace dependencies
try:
    import torch
    from transformers import AutoModelForImageTextToText, AutoProcessor

    HF_AVAILABLE = True
except ImportError:
    HF_AVAILABLE = False

from .models import load_model as load_model_handler


class HuggingFaceLocalAdapter(CustomLLM):
    """HuggingFace Local Adapter for running vision-language models locally."""

    def __init__(self, device: str = "auto", trust_remote_code: bool = False, **kwargs):
        """Initialize the adapter.

        Args:
            device: Device to load model on ("auto", "cuda", "cpu", etc.)
            trust_remote_code: Whether to trust remote code
            **kwargs: Additional arguments
        """
        super().__init__()
        self.device = device
        self.trust_remote_code = trust_remote_code
        # Cache for model handlers keyed by model_name
        self._handlers: Dict[str, Any] = {}
        self._executor = ThreadPoolExecutor(max_workers=1)  # Single thread pool

    def _get_handler(self, model_name: str):
        """Get or create a model handler for the given model name."""
        if model_name not in self._handlers:
            self._handlers[model_name] = load_model_handler(
                model_name=model_name, device=self.device, trust_remote_code=self.trust_remote_code
            )
        return self._handlers[model_name]

    def _convert_messages(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Convert OpenAI format messages to HuggingFace format.

        Args:
            messages: Messages in OpenAI format

        Returns:
            Messages in HuggingFace format
        """
        converted_messages = []

        for message in messages:
            converted_message = {"role": message["role"], "content": []}

            content = message.get("content", [])
            if isinstance(content, str):
                # Simple text content
                converted_message["content"].append({"type": "text", "text": content})
            elif isinstance(content, list):
                # Multi-modal content
                for item in content:
                    if item.get("type") == "text":
                        converted_message["content"].append(
                            {"type": "text", "text": item.get("text", "")}
                        )
                    elif item.get("type") == "image_url":
                        # Convert image_url format to image format
                        image_url = item.get("image_url", {}).get("url", "")
                        converted_message["content"].append({"type": "image", "image": image_url})

            converted_messages.append(converted_message)

        return converted_messages

    def _generate(self, **kwargs) -> str:
        """Generate response using the local HuggingFace model.

        Args:
            **kwargs: Keyword arguments containing messages and model info

        Returns:
            Generated text response
        """
        if not HF_AVAILABLE:
            raise ImportError(
                "HuggingFace transformers dependencies not found. "
                'Please install with: pip install "cua-agent[uitars-hf]"'
            )

        # Extract messages and model from kwargs
        messages = kwargs.get("messages", [])
        model_name = kwargs.get("model", "ByteDance-Seed/UI-TARS-1.5-7B")
        max_new_tokens = kwargs.get("max_tokens", 128)

        # Warn about ignored kwargs
        ignored_kwargs = set(kwargs.keys()) - {"messages", "model", "max_tokens"}
        if ignored_kwargs:
            warnings.warn(f"Ignoring unsupported kwargs: {ignored_kwargs}")

        # Convert messages to HuggingFace format
        hf_messages = self._convert_messages(messages)

        # Delegate to model handler
        handler = self._get_handler(model_name)
        generated_text = handler.generate(hf_messages, max_new_tokens=max_new_tokens)
        return generated_text

    def completion(self, *args, **kwargs) -> ModelResponse:
        """Synchronous completion method.

        Returns:
            ModelResponse with generated text
        """
        generated_text = self._generate(**kwargs)

        return completion(
            model=f"huggingface-local/{kwargs['model']}",
            mock_response=generated_text,
        )

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        """Asynchronous completion method.

        Returns:
            ModelResponse with generated text
        """
        # Run _generate in thread pool to avoid blocking
        loop = asyncio.get_event_loop()
        generated_text = await loop.run_in_executor(
            self._executor, functools.partial(self._generate, **kwargs)
        )

        return await acompletion(
            model=f"huggingface-local/{kwargs['model']}",
            mock_response=generated_text,
        )

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        """Synchronous streaming method.

        Returns:
            Iterator of GenericStreamingChunk
        """
        generated_text = self._generate(**kwargs)

        generic_streaming_chunk: GenericStreamingChunk = {
            "finish_reason": "stop",
            "index": 0,
            "is_finished": True,
            "text": generated_text,
            "tool_use": None,
            "usage": {"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0},
        }

        yield generic_streaming_chunk

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        """Asynchronous streaming method.

        Returns:
            AsyncIterator of GenericStreamingChunk
        """
        # Run _generate in thread pool to avoid blocking
        loop = asyncio.get_event_loop()
        generated_text = await loop.run_in_executor(
            self._executor, functools.partial(self._generate, **kwargs)
        )

        generic_streaming_chunk: GenericStreamingChunk = {
            "finish_reason": "stop",
            "index": 0,
            "is_finished": True,
            "text": generated_text,
            "tool_use": None,
            "usage": {"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0},
        }

        yield generic_streaming_chunk
