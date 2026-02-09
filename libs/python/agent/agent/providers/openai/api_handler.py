"""API handler for the OpenAI provider."""

import logging
import os
from datetime import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Optional

import requests

if TYPE_CHECKING:
    from .loop import OpenAILoop

logger = logging.getLogger(__name__)


class OpenAIAPIHandler:
    """Handler for OpenAI API interactions."""

    def __init__(self, loop: "OpenAILoop"):
        """Initialize the API handler.

        Args:
            loop: OpenAI loop instance
        """
        self.loop = loop
        self.api_key = os.getenv("OPENAI_API_KEY")
        if not self.api_key:
            raise ValueError("OPENAI_API_KEY environment variable not set")

        self.api_base = "https://api.openai.com/v1"
        self.headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
        }

        # Add organization if specified
        org_id = os.getenv("OPENAI_ORG")
        if org_id:
            self.headers["OpenAI-Organization"] = org_id

        logger.info("Initialized OpenAI API handler")

    async def send_initial_request(
        self,
        messages: List[Dict[str, Any]],
        display_width: str,
        display_height: str,
        previous_response_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Send an initial request to the OpenAI API with a screenshot.

        Args:
            messages: List of message objects in standard format
            display_width: Width of the display in pixels
            display_height: Height of the display in pixels
            previous_response_id: Optional ID of the previous response to link requests

        Returns:
            API response
        """
        # Convert display dimensions to integers
        try:
            width = int(display_width)
            height = int(display_height)
        except (ValueError, TypeError) as e:
            logger.error(f"Failed to convert display dimensions to integers: {str(e)}")
            raise ValueError(
                f"Display dimensions must be integers: width={display_width}, height={display_height}"
            )

        # Extract the latest text message and screenshot from messages
        latest_text = None
        latest_screenshot = None

        for msg in reversed(messages):
            if not isinstance(msg, dict):
                continue

            content = msg.get("content", [])

            if isinstance(content, str) and not latest_text:
                latest_text = content
                continue

            if not isinstance(content, list):
                continue

            for item in content:
                if not isinstance(item, dict):
                    continue

                # Look for text if we don't have it yet
                if not latest_text and item.get("type") == "text" and "text" in item:
                    latest_text = item.get("text", "")

                # Look for an image if we don't have it yet
                if not latest_screenshot and item.get("type") == "image":
                    source = item.get("source", {})
                    if source.get("type") == "base64" and "data" in source:
                        latest_screenshot = source["data"]

        # Prepare the input array
        input_array = []

        # Add the text message if found
        if latest_text:
            input_array.append({"role": "user", "content": latest_text})

        # Add the screenshot if found and no previous_response_id is provided
        if latest_screenshot and not previous_response_id:
            input_array.append(
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {
                            "type": "input_image",
                            "image_url": f"data:image/png;base64,{latest_screenshot}",
                        }
                    ],
                }
            )

        # Prepare the request payload - using minimal format from docs
        payload = {
            "model": "computer-use-preview",
            "tools": [
                {
                    "type": "computer_use_preview",
                    "display_width": width,
                    "display_height": height,
                    "environment": "mac",  # We're on macOS
                }
            ],
            "input": input_array,
            "reasoning": {
                "generate_summary": "concise",
            },
            "truncation": "auto",
        }

        # Add previous_response_id if provided
        if previous_response_id:
            payload["previous_response_id"] = previous_response_id

        # Log the request using the BaseLoop's log_api_call method
        self.loop._log_api_call("request", payload)

        # Log for debug purposes
        logger.info("Sending initial request to OpenAI API")
        logger.debug(f"Request payload: {self._sanitize_response(payload)}")

        # Send the request
        response = requests.post(
            f"{self.api_base}/responses",
            headers=self.headers,
            json=payload,
        )

        if response.status_code != 200:
            error_message = f"OpenAI API error: {response.status_code} {response.text}"
            logger.error(error_message)
            # Log the error using the BaseLoop's log_api_call method
            self.loop._log_api_call("error", payload, error=Exception(error_message))
            raise Exception(error_message)

        response_data = response.json()

        # Log the response using the BaseLoop's log_api_call method
        self.loop._log_api_call("response", payload, response_data)

        # Log for debug purposes
        logger.info("Received response from OpenAI API")
        logger.debug(f"Response data: {self._sanitize_response(response_data)}")

        return response_data

    async def send_computer_call_request(
        self,
        messages: List[Dict[str, Any]],
        display_width: str,
        display_height: str,
        previous_response_id: str,
    ) -> Dict[str, Any]:
        """Send a request to the OpenAI API with computer_call_output.

        Args:
            messages: List of message objects in standard format
            display_width: Width of the display in pixels
            display_height: Height of the display in pixels
            system_prompt: System prompt to include
            previous_response_id: ID of the previous response to link requests

        Returns:
            API response
        """
        # Convert display dimensions to integers
        try:
            width = int(display_width)
            height = int(display_height)
        except (ValueError, TypeError) as e:
            logger.error(f"Failed to convert display dimensions to integers: {str(e)}")
            raise ValueError(
                f"Display dimensions must be integers: width={display_width}, height={display_height}"
            )

        # Find the most recent computer_call_output with call_id
        call_id = None
        screenshot_base64 = None

        # Look for call_id and screenshot in messages
        for msg in reversed(messages):
            if not isinstance(msg, dict):
                continue

            # Check if the message itself has a call_id
            if "call_id" in msg and not call_id:
                call_id = msg["call_id"]

            content = msg.get("content", [])
            if not isinstance(content, list):
                continue

            for item in content:
                if not isinstance(item, dict):
                    continue

                # Look for call_id
                if not call_id and "call_id" in item:
                    call_id = item["call_id"]

                # Look for screenshot in computer_call_output
                if not screenshot_base64 and item.get("type") == "computer_call_output":
                    output = item.get("output", {})
                    if isinstance(output, dict) and "image_url" in output:
                        image_url = output.get("image_url", "")
                        if image_url.startswith("data:image/png;base64,"):
                            screenshot_base64 = image_url[len("data:image/png;base64,") :]

                # Look for screenshot in image type
                if not screenshot_base64 and item.get("type") == "image":
                    source = item.get("source", {})
                    if source.get("type") == "base64" and "data" in source:
                        screenshot_base64 = source["data"]

        if not call_id or not screenshot_base64:
            logger.error("Missing call_id or screenshot for computer_call_output")
            logger.error(f"Last message: {messages[-1] if messages else None}")
            raise ValueError("Cannot create computer call request: missing call_id or screenshot")

        # Prepare the request payload using minimal format from docs
        payload = {
            "model": "computer-use-preview",
            "previous_response_id": previous_response_id,
            "tools": [
                {
                    "type": "computer_use_preview",
                    "display_width": width,
                    "display_height": height,
                    "environment": "mac",  # We're on macOS
                }
            ],
            "input": [
                {
                    "type": "computer_call_output",
                    "call_id": call_id,
                    "output": {
                        "type": "input_image",
                        "image_url": f"data:image/png;base64,{screenshot_base64}",
                    },
                }
            ],
            "truncation": "auto",
        }

        # Log the request using the BaseLoop's log_api_call method
        self.loop._log_api_call("request", payload)

        # Log for debug purposes
        logger.info("Sending computer call request to OpenAI API")
        logger.debug(f"Request payload: {self._sanitize_response(payload)}")

        # Send the request
        response = requests.post(
            f"{self.api_base}/responses",
            headers=self.headers,
            json=payload,
        )

        if response.status_code != 200:
            error_message = f"OpenAI API error: {response.status_code} {response.text}"
            logger.error(error_message)
            # Log the error using the BaseLoop's log_api_call method
            self.loop._log_api_call("error", payload, error=Exception(error_message))
            raise Exception(error_message)

        response_data = response.json()

        # Log the response using the BaseLoop's log_api_call method
        self.loop._log_api_call("response", payload, response_data)

        # Log for debug purposes
        logger.info("Received response from OpenAI API")
        logger.debug(f"Response data: {self._sanitize_response(response_data)}")

        return response_data

    def _format_messages_for_agent_response(
        self, messages: List[Dict[str, Any]]
    ) -> List[Dict[str, Any]]:
        """Format messages for the OpenAI Agent Response API.

        The Agent Response API requires specific content types:
        - For user messages: use "input_text", "input_image", etc.
        - For assistant messages: use "output_text" only

        Additionally, when using the computer tool, only one image can be sent.

        Args:
            messages: List of standard messages

        Returns:
            Messages formatted for the Agent Response API
        """
        formatted_messages = []
        has_image = False  # Track if we've already included an image

        # We need to process messages in reverse to ensure we keep the most recent image
        # but preserve the original order in the final output
        reversed_messages = list(reversed(messages))
        temp_formatted = []

        for msg in reversed_messages:
            if not msg:
                continue

            role = msg.get("role", "user")
            content = msg.get("content", "")

            logger.debug(f"Processing message - Role: {role}, Content type: {type(content)}")
            if isinstance(content, list):
                logger.debug(
                    f"List content items: {[item.get('type') for item in content if isinstance(item, dict)]}"
                )

            if isinstance(content, str):
                # For string content, create a message with the appropriate text type
                if role == "user":
                    temp_formatted.append(
                        {"role": role, "content": [{"type": "input_text", "text": content}]}
                    )
                elif role == "assistant":
                    # For assistant, we need explicit output_text
                    temp_formatted.append(
                        {"role": role, "content": [{"type": "output_text", "text": content}]}
                    )
                elif role == "system":
                    # System messages need to be formatted as input_text as well
                    temp_formatted.append(
                        {"role": role, "content": [{"type": "input_text", "text": content}]}
                    )
            elif isinstance(content, list):
                # For list content, convert each item to the correct type based on role
                formatted_content = []
                has_image_in_this_message = False

                for item in content:
                    if not isinstance(item, dict):
                        continue

                    item_type = item.get("type")

                    if role == "user":
                        # Handle user message formatting
                        if item_type == "text" or item_type == "input_text":
                            # Text from user is input_text
                            formatted_content.append(
                                {"type": "input_text", "text": item.get("text", "")}
                            )
                        elif (item_type == "image" or item_type == "image_url") and not has_image:
                            # Only include the first/most recent image we encounter
                            if item_type == "image":
                                # Image from user is input_image
                                source = item.get("source", {})
                                if source.get("type") == "base64" and "data" in source:
                                    formatted_content.append(
                                        {
                                            "type": "input_image",
                                            "image_url": f"data:image/png;base64,{source['data']}",
                                        }
                                    )
                                    has_image = True
                                    has_image_in_this_message = True
                            elif item_type == "image_url":
                                # Convert "image_url" to "input_image"
                                formatted_content.append(
                                    {
                                        "type": "input_image",
                                        "image_url": item.get("image_url", {}).get("url", ""),
                                    }
                                )
                                has_image = True
                                has_image_in_this_message = True
                    elif role == "assistant":
                        # Handle assistant message formatting - only output_text is supported
                        if item_type == "text" or item_type == "output_text":
                            formatted_content.append(
                                {"type": "output_text", "text": item.get("text", "")}
                            )

                if formatted_content:
                    # If this message had an image, mark it for inclusion
                    temp_formatted.append(
                        {
                            "role": role,
                            "content": formatted_content,
                            "_had_image": has_image_in_this_message,  # Temporary marker
                        }
                    )

        # Reverse back to original order and cleanup
        for msg in reversed(temp_formatted):
            # Remove our temporary marker
            if "_had_image" in msg:
                del msg["_had_image"]
            formatted_messages.append(msg)

        # Log summary for debugging
        num_images = sum(
            1
            for msg in formatted_messages
            for item in (msg.get("content", []) if isinstance(msg.get("content"), list) else [])
            if isinstance(item, dict) and item.get("type") == "input_image"
        )
        logger.info(f"Formatted {len(messages)} messages for OpenAI API with {num_images} images")

        return formatted_messages

    def _sanitize_response(self, response: Dict[str, Any]) -> Dict[str, Any]:
        """Sanitize response for logging by removing large image data.

        Args:
            response: Response to sanitize

        Returns:
            Sanitized response
        """
        from .utils import sanitize_message

        # Deep copy to avoid modifying the original
        sanitized = response.copy()

        # Sanitize output items if present
        if "output" in sanitized and isinstance(sanitized["output"], list):
            sanitized["output"] = [sanitize_message(item) for item in sanitized["output"]]

        return sanitized
