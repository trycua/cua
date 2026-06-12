"""
Base callback handler interface for ComputerAgent preprocessing and postprocessing hooks.
"""

from abc import ABC, abstractmethod
from typing import Any, Dict, List, Optional, Union


class AsyncCallbackHandler(ABC):
    """
    Base class for async callback handlers that can preprocess messages before
    the agent loop and postprocess output after the agent loop.
    """

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Called at the start of an agent run loop."""
        pass

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        """Called at the end of an agent run loop."""
        pass

    async def on_run_continue(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> bool:
        """Called during agent run loop to determine if execution should continue.

        Args:
            kwargs: Run arguments
            old_items: Original messages
            new_items: New messages generated during run

        Returns:
            True to continue execution, False to stop
        """
        return True

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """
        Called before messages are sent to the agent loop.

        Args:
            messages: List of message dictionaries to preprocess

        Returns:
            List of preprocessed message dictionaries
        """
        return messages

    async def on_llm_end(self, output: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """
        Called after the agent loop returns output.

        Args:
            output: List of output message dictionaries to postprocess

        Returns:
            List of postprocessed output dictionaries
        """
        return output

    async def on_computer_call_start(self, item: Dict[str, Any]) -> None:
        """
        Called when a computer call is about to start.

        Args:
            item: The computer call item dictionary
        """
        pass

    async def on_computer_call_end(
        self, item: Dict[str, Any], result: List[Dict[str, Any]]
    ) -> None:
        """
        Called when a computer call has completed.

        Args:
            item: The computer call item dictionary
            result: The result of the computer call
        """
        pass

    async def on_function_call_start(self, item: Dict[str, Any]) -> None:
        """
        Called when a function call is about to start.

        Args:
            item: The function call item dictionary
        """
        pass

    async def on_function_call_end(
        self, item: Dict[str, Any], result: List[Dict[str, Any]]
    ) -> None:
        """
        Called when a function call has completed.

        Args:
            item: The function call item dictionary
            result: The result of the function call
        """
        pass

    async def on_text(self, item: Dict[str, Any]) -> None:
        """
        Called when a text message is encountered.

        Args:
            item: The message item dictionary
        """
        pass

    async def on_api_start(self, kwargs: Dict[str, Any]) -> None:
        """
        Called when an API call is about to start.

        Args:
            kwargs: The kwargs being passed to the API call
        """
        pass

    async def on_api_end(self, kwargs: Dict[str, Any], result: Any) -> None:
        """
        Called when an API call has completed.

        Args:
            kwargs: The kwargs that were passed to the API call
            result: The result of the API call
        """
        pass

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """
        Called when usage information is received.

        Args:
            usage: The usage information
        """
        pass

    async def on_screenshot(self, screenshot: Union[str, bytes], name: str = "screenshot") -> None:
        """
        Called when a screenshot is taken.

        Args:
            screenshot: The screenshot image
            name: The name of the screenshot
        """
        pass

    async def on_responses(self, kwargs: Dict[str, Any], responses: Dict[str, Any]) -> None:
        """
        Called when responses are received.

        Args:
            kwargs: The kwargs being passed to the agent loop
            responses: The responses received
        """
        pass
