"""
Logging callback for ComputerAgent that provides configurable logging of agent lifecycle events.
"""

import json
import logging
from typing import Any, Dict, List, Optional, Union

from .base import AsyncCallbackHandler


def sanitize_image_urls(data: Any) -> Any:
    """
    Recursively search for 'image_url' keys and set their values to '[omitted]'.

    Args:
        data: Any data structure (dict, list, or primitive type)

    Returns:
        A deep copy of the data with all 'image_url' values replaced with '[omitted]'
    """
    if isinstance(data, dict):
        # Create a copy of the dictionary
        sanitized = {}
        for key, value in data.items():
            if key == "image_url":
                sanitized[key] = "[omitted]"
            else:
                # Recursively sanitize the value
                sanitized[key] = sanitize_image_urls(value)
        return sanitized

    elif isinstance(data, list):
        # Recursively sanitize each item in the list
        return [sanitize_image_urls(item) for item in data]

    else:
        # For primitive types (str, int, bool, None, etc.), return as-is
        return data


class LoggingCallback(AsyncCallbackHandler):
    """
    Callback handler that logs agent lifecycle events with configurable verbosity.

    Logging levels:
    - DEBUG: All events including API calls, message preprocessing, and detailed outputs
    - INFO: Major lifecycle events (start/end, messages, outputs)
    - WARNING: Only warnings and errors
    - ERROR: Only errors
    """

    def __init__(self, logger: Optional[logging.Logger] = None, level: int = logging.INFO):
        """
        Initialize the logging callback.

        Args:
            logger: Logger instance to use. If None, creates a logger named 'agent.ComputerAgent'
            level: Logging level (logging.DEBUG, logging.INFO, etc.)
        """
        self.logger = logger or logging.getLogger("agent.ComputerAgent")
        self.level = level

        # Set up logger if it doesn't have handlers
        if not self.logger.handlers:
            handler = logging.StreamHandler()
            formatter = logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
            handler.setFormatter(formatter)
            self.logger.addHandler(handler)
            self.logger.setLevel(level)

    def _update_usage(self, usage: Dict[str, Any]) -> None:
        """Update total usage statistics."""

        def add_dicts(target: Dict[str, Any], source: Dict[str, Any]) -> None:
            for key, value in source.items():
                if isinstance(value, dict):
                    if key not in target:
                        target[key] = {}
                    add_dicts(target[key], value)
                else:
                    if key not in target:
                        target[key] = 0
                    target[key] += value

        add_dicts(self.total_usage, usage)

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Called before the run starts."""
        self.total_usage = {}

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Called when usage information is received."""
        self._update_usage(usage)

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        """Called after the run ends."""

        def format_dict(d, indent=0):
            lines = []
            prefix = f" - {' ' * indent}"
            for key, value in d.items():
                if isinstance(value, dict):
                    lines.append(f"{prefix}{key}:")
                    lines.extend(format_dict(value, indent + 1))
                elif isinstance(value, float):
                    lines.append(f"{prefix}{key}: ${value:.4f}")
                else:
                    lines.append(f"{prefix}{key}: {value}")
            return lines

        formatted_output = "\n".join(format_dict(self.total_usage))
        self.logger.info(f"Total usage:\n{formatted_output}")

    async def on_llm_start(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Called before LLM processing starts."""
        if self.logger.isEnabledFor(logging.INFO):
            self.logger.info(f"LLM processing started with {len(messages)} messages")
        if self.logger.isEnabledFor(logging.DEBUG):
            sanitized_messages = [sanitize_image_urls(msg) for msg in messages]
            self.logger.debug(f"LLM input messages: {json.dumps(sanitized_messages, indent=2)}")
        return messages

    async def on_llm_end(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Called after LLM processing ends."""
        if self.logger.isEnabledFor(logging.DEBUG):
            sanitized_messages = [sanitize_image_urls(msg) for msg in messages]
            self.logger.debug(f"LLM output: {json.dumps(sanitized_messages, indent=2)}")
        return messages

    async def on_computer_call_start(self, item: Dict[str, Any]) -> None:
        """Called when a computer call starts."""
        action = item.get("action", {})
        action_type = action.get("type", "unknown")
        action_args = {k: v for k, v in action.items() if k != "type"}

        # INFO level logging for the action
        self.logger.info(f"Computer: {action_type}({action_args})")

        # DEBUG level logging for full details
        if self.logger.isEnabledFor(logging.DEBUG):
            self.logger.debug(f"Computer call started: {json.dumps(action, indent=2)}")

    async def on_computer_call_end(self, item: Dict[str, Any], result: Any) -> None:
        """Called when a computer call ends."""
        if self.logger.isEnabledFor(logging.DEBUG):
            action = item.get("action", "unknown")
            self.logger.debug(f"Computer call completed: {json.dumps(action, indent=2)}")
            if result:
                sanitized_result = sanitize_image_urls(result)
                self.logger.debug(f"Computer call result: {json.dumps(sanitized_result, indent=2)}")

    async def on_function_call_start(self, item: Dict[str, Any]) -> None:
        """Called when a function call starts."""
        name = item.get("name", "unknown")
        arguments = item.get("arguments", "{}")

        # INFO level logging for the function call
        self.logger.info(f"Function: {name}({arguments})")

        # DEBUG level logging for full details
        if self.logger.isEnabledFor(logging.DEBUG):
            self.logger.debug(f"Function call started: {name}")

    async def on_function_call_end(self, item: Dict[str, Any], result: Any) -> None:
        """Called when a function call ends."""
        # INFO level logging for function output (similar to function_call_output)
        if result:
            # Handle both list and direct result formats
            if isinstance(result, list) and len(result) > 0:
                output = (
                    result[0].get("output", str(result))
                    if isinstance(result[0], dict)
                    else str(result[0])
                )
            else:
                output = str(result)

            # Truncate long outputs
            if len(output) > 100:
                output = output[:100] + "..."

            self.logger.info(f"Output: {output}")

        # DEBUG level logging for full details
        if self.logger.isEnabledFor(logging.DEBUG):
            name = item.get("name", "unknown")
            self.logger.debug(f"Function call completed: {name}")
            if result:
                self.logger.debug(f"Function call result: {json.dumps(result, indent=2)}")

    async def on_text(self, item: Dict[str, Any]) -> None:
        """Called when a text message is encountered."""
        # Get the role to determine if it's Agent or User
        role = item.get("role", "unknown")
        content_items = item.get("content", [])

        # Process content items to build display text
        text_parts = []
        for content_item in content_items:
            content_type = content_item.get("type", "output_text")
            if content_type == "output_text":
                text_content = content_item.get("text", "")
                if not text_content.strip():
                    text_parts.append("[empty]")
                else:
                    # Truncate long text and add ellipsis
                    if len(text_content) > 2048:
                        text_parts.append(text_content[:2048] + "...")
                    else:
                        text_parts.append(text_content)
            else:
                # Non-text content, show as [type]
                text_parts.append(f"[{content_type}]")

        # Join all text parts
        display_text = "".join(text_parts) if text_parts else "[empty]"

        # Log with appropriate level and format
        if role == "assistant":
            self.logger.info(f"Agent: {display_text}")
        elif role == "user":
            self.logger.info(f"User: {display_text}")
        else:
            # Fallback for unknown roles, use debug level
            if self.logger.isEnabledFor(logging.DEBUG):
                self.logger.debug(f"Text message ({role}): {display_text}")

    async def on_api_start(self, kwargs: Dict[str, Any]) -> None:
        """Called when an API call is about to start."""
        if self.logger.isEnabledFor(logging.DEBUG):
            model = kwargs.get("model", "unknown")
            self.logger.debug(f"API call starting for model: {model}")
            # Log sanitized messages if present
            if "messages" in kwargs:
                sanitized_messages = sanitize_image_urls(kwargs["messages"])
                self.logger.debug(f"API call messages: {json.dumps(sanitized_messages, indent=2)}")
            elif "input" in kwargs:
                sanitized_input = sanitize_image_urls(kwargs["input"])
                self.logger.debug(f"API call input: {json.dumps(sanitized_input, indent=2)}")

    async def on_api_end(self, kwargs: Dict[str, Any], result: Any) -> None:
        """Called when an API call has completed."""
        if self.logger.isEnabledFor(logging.DEBUG):
            model = kwargs.get("model", "unknown")
            self.logger.debug(f"API call completed for model: {model}")
            self.logger.debug(
                f"API call result: {json.dumps(sanitize_image_urls(result), indent=2)}"
            )

    async def on_screenshot(self, item: Union[str, bytes], name: str = "screenshot") -> None:
        """Called when a screenshot is taken."""
        if self.logger.isEnabledFor(logging.DEBUG):
            image_size = len(item) / 1024
            self.logger.debug(f"Screenshot captured: {name} {image_size:.2f} KB")
