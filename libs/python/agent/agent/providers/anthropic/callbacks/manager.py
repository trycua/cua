from typing import Callable, Protocol

import httpx
from anthropic.types.beta import BetaContentBlockParam

from ..tools import ToolResult


class APICallback(Protocol):
    """Protocol for API callbacks."""

    def __call__(
        self,
        request: httpx.Request | None,
        response: httpx.Response | object | None,
        error: Exception | None,
    ) -> None: ...


class ContentCallback(Protocol):
    """Protocol for content callbacks."""

    def __call__(self, content: BetaContentBlockParam) -> None: ...


class ToolCallback(Protocol):
    """Protocol for tool callbacks."""

    def __call__(self, result: ToolResult, tool_id: str) -> None: ...


class CallbackManager:
    """Manages various callbacks for the agent system."""

    def __init__(
        self,
        content_callback: ContentCallback,
        tool_callback: ToolCallback,
        api_callback: APICallback,
    ):
        """Initialize the callback manager.

        Args:
            content_callback: Callback for content updates
            tool_callback: Callback for tool execution results
            api_callback: Callback for API interactions
        """
        self.content_callback = content_callback
        self.tool_callback = tool_callback
        self.api_callback = api_callback

    def on_content(self, content: BetaContentBlockParam) -> None:
        """Handle content updates."""
        self.content_callback(content)

    def on_tool_result(self, result: ToolResult, tool_id: str) -> None:
        """Handle tool execution results."""
        self.tool_callback(result, tool_id)

    def on_api_interaction(
        self,
        request: httpx.Request | None,
        response: httpx.Response | object | None,
        error: Exception | None,
    ) -> None:
        """Handle API interactions."""
        self.api_callback(request, response, error)
