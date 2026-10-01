import asyncio
import os
from typing import Any, AsyncIterator, Dict, Iterator, List

import requests
from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse


class HumanAdapter(CustomLLM):
    """Human Adapter for human-in-the-loop completions.

    This adapter sends completion requests to a human completion server
    where humans can review and respond to AI requests.
    """

    def __init__(self, base_url: str | None = None, timeout: float = 300.0, **kwargs):
        """Initialize the human adapter.

        Args:
            base_url: Base URL for the human completion server.
                     Defaults to HUMAN_BASE_URL environment variable or http://localhost:8002
            timeout: Timeout in seconds for waiting for human response
            **kwargs: Additional arguments
        """
        super().__init__()
        self.base_url = base_url or os.getenv("HUMAN_BASE_URL", "http://localhost:8002")
        self.timeout = timeout

        # Ensure base_url doesn't end with slash
        self.base_url = self.base_url.rstrip("/")

    def _queue_completion(self, messages: List[Dict[str, Any]], model: str) -> str:
        """Queue a completion request and return the call ID.

        Args:
            messages: Messages in OpenAI format
            model: Model name

        Returns:
            Call ID for tracking the request

        Raises:
            Exception: If queueing fails
        """
        try:
            response = requests.post(
                f"{self.base_url}/queue", json={"messages": messages, "model": model}, timeout=10
            )
            response.raise_for_status()
            return response.json()["id"]
        except requests.RequestException as e:
            raise Exception(f"Failed to queue completion request: {e}")

    def _wait_for_completion(self, call_id: str) -> Dict[str, Any]:
        """Wait for human to complete the call.

        Args:
            call_id: ID of the queued completion call

        Returns:
            Dict containing response and/or tool_calls

        Raises:
            TimeoutError: If timeout is exceeded
            Exception: If completion fails
        """
        import time

        start_time = time.time()

        while True:
            try:
                # Check status
                status_response = requests.get(f"{self.base_url}/status/{call_id}")
                status_response.raise_for_status()
                status_data = status_response.json()

                if status_data["status"] == "completed":
                    result = {}
                    if "response" in status_data and status_data["response"]:
                        result["response"] = status_data["response"]
                    if "tool_calls" in status_data and status_data["tool_calls"]:
                        result["tool_calls"] = status_data["tool_calls"]
                    return result
                elif status_data["status"] == "failed":
                    error_msg = status_data.get("error", "Unknown error")
                    raise Exception(f"Completion failed: {error_msg}")

                # Check timeout
                if time.time() - start_time > self.timeout:
                    raise TimeoutError(
                        f"Timeout waiting for human response after {self.timeout} seconds"
                    )

                # Wait before checking again
                time.sleep(1.0)

            except requests.RequestException as e:
                if time.time() - start_time > self.timeout:
                    raise TimeoutError(f"Timeout waiting for human response: {e}")
                # Continue trying if we haven't timed out
                time.sleep(1.0)

    async def _async_wait_for_completion(self, call_id: str) -> Dict[str, Any]:
        """Async version of wait_for_completion.

        Args:
            call_id: ID of the queued completion call

        Returns:
            Dict containing response and/or tool_calls

        Raises:
            TimeoutError: If timeout is exceeded
            Exception: If completion fails
        """
        import time

        import aiohttp

        start_time = time.time()

        async with aiohttp.ClientSession() as session:
            while True:
                try:
                    # Check status
                    async with session.get(f"{self.base_url}/status/{call_id}") as response:
                        response.raise_for_status()
                        status_data = await response.json()

                    if status_data["status"] == "completed":
                        result = {}
                        if "response" in status_data and status_data["response"]:
                            result["response"] = status_data["response"]
                        if "tool_calls" in status_data and status_data["tool_calls"]:
                            result["tool_calls"] = status_data["tool_calls"]
                        return result
                    elif status_data["status"] == "failed":
                        error_msg = status_data.get("error", "Unknown error")
                        raise Exception(f"Completion failed: {error_msg}")

                    # Check timeout
                    if time.time() - start_time > self.timeout:
                        raise TimeoutError(
                            f"Timeout waiting for human response after {self.timeout} seconds"
                        )

                    # Wait before checking again
                    await asyncio.sleep(1.0)

                except Exception as e:
                    if time.time() - start_time > self.timeout:
                        raise TimeoutError(f"Timeout waiting for human response: {e}")
                    # Continue trying if we haven't timed out
                    await asyncio.sleep(1.0)

    def _generate_response(self, messages: List[Dict[str, Any]], model: str) -> Dict[str, Any]:
        """Generate a human response for the given messages.

        Args:
            messages: Messages in OpenAI format
            model: Model name

        Returns:
            Dict containing response and/or tool_calls
        """
        # Queue the completion request
        call_id = self._queue_completion(messages, model)

        # Wait for human response
        response = self._wait_for_completion(call_id)

        return response

    async def _async_generate_response(
        self, messages: List[Dict[str, Any]], model: str
    ) -> Dict[str, Any]:
        """Async version of _generate_response.

        Args:
            messages: Messages in OpenAI format
            model: Model name

        Returns:
            Dict containing response and/or tool_calls
        """
        # Queue the completion request (sync operation)
        call_id = self._queue_completion(messages, model)

        # Wait for human response (async)
        response = await self._async_wait_for_completion(call_id)

        return response

    def completion(self, *args, **kwargs) -> ModelResponse:
        """Synchronous completion method.

        Returns:
            ModelResponse with human-generated text or tool calls
        """
        messages = kwargs.get("messages", [])
        model = kwargs.get("model", "human")

        # Generate human response
        human_response_data = self._generate_response(messages, model)

        # Create ModelResponse with proper structure
        import time
        import uuid

        from litellm.types.utils import Choices, Message, ModelResponse

        # Create message content based on response type
        if "tool_calls" in human_response_data and human_response_data["tool_calls"]:
            # Tool calls response
            message = Message(
                role="assistant",
                content=human_response_data.get("response", ""),
                tool_calls=human_response_data["tool_calls"],
            )
        else:
            # Text response
            message = Message(role="assistant", content=human_response_data.get("response", ""))

        choice = Choices(finish_reason="stop", index=0, message=message)

        result = ModelResponse(
            id=f"human-{uuid.uuid4()}",
            choices=[choice],
            created=int(time.time()),
            model=f"human/{model}",
            object="chat.completion",
        )

        return result

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        """Asynchronous completion method.

        Returns:
            ModelResponse with human-generated text or tool calls
        """
        messages = kwargs.get("messages", [])
        model = kwargs.get("model", "human")

        # Generate human response
        human_response_data = await self._async_generate_response(messages, model)

        # Create ModelResponse with proper structure
        import time
        import uuid

        from litellm.types.utils import Choices, Message, ModelResponse

        # Create message content based on response type
        if "tool_calls" in human_response_data and human_response_data["tool_calls"]:
            # Tool calls response
            message = Message(
                role="assistant",
                content=human_response_data.get("response", ""),
                tool_calls=human_response_data["tool_calls"],
            )
        else:
            # Text response
            message = Message(role="assistant", content=human_response_data.get("response", ""))

        choice = Choices(finish_reason="stop", index=0, message=message)

        result = ModelResponse(
            id=f"human-{uuid.uuid4()}",
            choices=[choice],
            created=int(time.time()),
            model=f"human/{model}",
            object="chat.completion",
        )

        return result

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        """Synchronous streaming method.

        Yields:
            Streaming chunks with human-generated text or tool calls
        """
        messages = kwargs.get("messages", [])
        model = kwargs.get("model", "human")

        # Generate human response
        human_response_data = self._generate_response(messages, model)

        import time

        # Handle tool calls vs text response
        if "tool_calls" in human_response_data and human_response_data["tool_calls"]:
            # Stream tool calls as a single chunk
            generic_chunk: GenericStreamingChunk = {
                "finish_reason": "tool_calls",
                "index": 0,
                "is_finished": True,
                "text": human_response_data.get("response", ""),
                "tool_use": human_response_data["tool_calls"],
                "usage": {"completion_tokens": 1, "prompt_tokens": 0, "total_tokens": 1},
            }
            yield generic_chunk
        else:
            # Stream text response
            response_text = human_response_data.get("response", "")
            generic_chunk: GenericStreamingChunk = {
                "finish_reason": "stop",
                "index": 0,
                "is_finished": True,
                "text": response_text,
                "tool_use": None,
                "usage": {
                    "completion_tokens": len(response_text.split()),
                    "prompt_tokens": 0,
                    "total_tokens": len(response_text.split()),
                },
            }
            yield generic_chunk

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        """Asynchronous streaming method.

        Yields:
            Streaming chunks with human-generated text or tool calls
        """
        messages = kwargs.get("messages", [])
        model = kwargs.get("model", "human")

        # Generate human response
        human_response = await self._async_generate_response(messages, model)

        # Return as single streaming chunk
        generic_streaming_chunk: GenericStreamingChunk = {
            "finish_reason": "stop",
            "index": 0,
            "is_finished": True,
            "text": human_response,
            "tool_use": None,
            "usage": {
                "completion_tokens": len(human_response.split()),
                "prompt_tokens": 0,
                "total_tokens": len(human_response.split()),
            },
        }

        yield generic_streaming_chunk
