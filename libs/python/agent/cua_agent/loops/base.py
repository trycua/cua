"""
Base protocol for async agent configurations
"""

from abc import abstractmethod
from typing import Any, Dict, List, Optional, Protocol, Tuple, Union

from ..types import AgentCapability


class AsyncAgentConfig(Protocol):
    """Protocol defining the interface for async agent configurations."""

    @abstractmethod
    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler=None,
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **generation_config,
    ) -> Dict[str, Any]:
        """
        Predict the next step based on input items.

        Args:
            messages: Input items following Responses format (message, function_call, computer_call)
            model: Model name to use
            tools: Optional list of tool schemas
            max_retries: Maximum number of retries for failed API calls
            stream: Whether to stream responses
            computer_handler: Computer handler instance
            _on_api_start: Callback for API start
            _on_api_end: Callback for API end
            _on_usage: Callback for usage tracking
            _on_screenshot: Callback for screenshot events
            **generation_config: Additional arguments to pass to the model provider
                - api_key: Optional API key for the provider
                - api_base: Optional API base URL for the provider

        Returns:
            Dictionary with "output" (output items) and "usage" array
        """
        ...

    @abstractmethod
    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **generation_config
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates based on image and instruction.

        Args:
            model: Model name to use
            image_b64: Base64 encoded image
            instruction: Instruction for where to click
            **generation_config: Additional arguments to pass to the model provider
                - api_key: Optional API key for the provider
                - api_base: Optional API base URL for the provider

        Returns:
            None or tuple with (x, y) coordinates
        """
        ...

    @abstractmethod
    def get_capabilities(self) -> List[AgentCapability]:
        """
        Get list of capabilities supported by this agent config.

        Returns:
            List of capability strings (e.g., ["step", "click"])
        """
        ...
