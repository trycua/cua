"""Core experiment management for agents."""

import base64
import json
import logging
import os
import re
from datetime import datetime
from io import BytesIO
from typing import Any, Dict, List, Optional

from PIL import Image

logger = logging.getLogger(__name__)


class ExperimentManager:
    """Manages experiment directories and logging for the agent."""

    def __init__(
        self,
        base_dir: Optional[str] = None,
        only_n_most_recent_images: Optional[int] = None,
    ):
        """Initialize the experiment manager.

        Args:
            base_dir: Base directory for saving experiment data
            only_n_most_recent_images: Maximum number of recent screenshots to include in API requests
        """
        self.base_dir = base_dir
        self.only_n_most_recent_images = only_n_most_recent_images
        self.run_dir = None
        self.current_turn_dir = None
        self.turn_count = 0
        self.screenshot_count = 0
        # Track all screenshots for potential API request inclusion
        self.screenshot_paths = []

        # Set up experiment directories if base_dir is provided
        if self.base_dir:
            self.setup_experiment_dirs()

    def setup_experiment_dirs(self) -> None:
        """Setup the experiment directory structure."""
        if not self.base_dir:
            return

        # Create base experiments directory if it doesn't exist
        os.makedirs(self.base_dir, exist_ok=True)

        # Create timestamped run directory
        timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        self.run_dir = os.path.join(self.base_dir, timestamp)
        os.makedirs(self.run_dir, exist_ok=True)
        logger.info(f"Created run directory: {self.run_dir}")

        # Create first turn directory
        self.create_turn_dir()

    def create_turn_dir(self) -> None:
        """Create a new directory for the current turn."""
        if not self.run_dir:
            logger.warning("Cannot create turn directory: run_dir not set")
            return

        # Increment turn counter
        self.turn_count += 1

        # Create turn directory with padded number
        turn_name = f"turn_{self.turn_count:03d}"
        self.current_turn_dir = os.path.join(self.run_dir, turn_name)
        os.makedirs(self.current_turn_dir, exist_ok=True)
        logger.info(f"Created turn directory: {self.current_turn_dir}")

    def sanitize_log_data(self, data: Any) -> Any:
        """Sanitize log data by replacing large binary data with placeholders.

        Args:
            data: Data to sanitize

        Returns:
            Sanitized copy of the data
        """
        if isinstance(data, dict):
            result = {}
            for k, v in data.items():
                # Special handling for 'data' field in Anthropic message source
                if k == "data" and isinstance(v, str) and len(v) > 1000:
                    result[k] = f"[BASE64_DATA_LENGTH_{len(v)}]"
                # Special handling for the 'media_type' key which indicates we're in an image block
                elif k == "media_type" and "image" in str(v):
                    result[k] = v
                    # If we're in an image block, look for a sibling 'data' field with base64 content
                    if (
                        "data" in result
                        and isinstance(result["data"], str)
                        and len(result["data"]) > 1000
                    ):
                        result["data"] = f"[BASE64_DATA_LENGTH_{len(result['data'])}]"
                else:
                    result[k] = self.sanitize_log_data(v)
            return result
        elif isinstance(data, list):
            return [self.sanitize_log_data(item) for item in data]
        elif isinstance(data, str) and len(data) > 1000 and "base64" in data.lower():
            return f"[BASE64_DATA_LENGTH_{len(data)}]"
        else:
            return data

    def save_screenshot(self, img_base64: str, action_type: str = "") -> Optional[str]:
        """Save a screenshot to the experiment directory.

        Args:
            img_base64: Base64 encoded screenshot
            action_type: Type of action that triggered the screenshot

        Returns:
            Path to the saved screenshot or None if there was an error
        """
        if not self.current_turn_dir:
            return None

        try:
            # Increment screenshot counter
            self.screenshot_count += 1

            # Sanitize action_type to ensure valid filename
            # Replace characters that are not safe for filenames
            sanitized_action = ""
            if action_type:
                # Replace invalid filename characters with underscores
                sanitized_action = re.sub(r'[\\/*?:"<>|]', "_", action_type)
                # Limit the length to avoid excessively long filenames
                sanitized_action = sanitized_action[:50]

            # Create a descriptive filename
            timestamp = int(datetime.now().timestamp() * 1000)
            action_suffix = f"_{sanitized_action}" if sanitized_action else ""
            filename = f"screenshot_{self.screenshot_count:03d}{action_suffix}_{timestamp}.png"

            # Save directly to the turn directory
            filepath = os.path.join(self.current_turn_dir, filename)

            # Save the screenshot
            img_data = base64.b64decode(img_base64)
            with open(filepath, "wb") as f:
                f.write(img_data)

            # Keep track of the file path
            self.screenshot_paths.append(filepath)

            return filepath
        except Exception as e:
            logger.error(f"Error saving screenshot: {str(e)}")
            return None

    def save_action_visualization(
        self, img: Image.Image, action_name: str, details: str = ""
    ) -> str:
        """Save a visualization of an action.

        Args:
            img: Image to save
            action_name: Name of the action
            details: Additional details about the action

        Returns:
            Path to the saved image
        """
        if not self.current_turn_dir:
            return ""

        try:
            # Create a descriptive filename
            timestamp = int(datetime.now().timestamp() * 1000)
            details_suffix = f"_{details}" if details else ""
            filename = f"vis_{action_name}{details_suffix}_{timestamp}.png"

            # Save directly to the turn directory
            filepath = os.path.join(self.current_turn_dir, filename)

            # Save the image
            img.save(filepath)

            # Keep track of the file path
            self.screenshot_paths.append(filepath)

            return filepath
        except Exception as e:
            logger.error(f"Error saving action visualization: {str(e)}")
            return ""

    def log_api_call(
        self,
        call_type: str,
        request: Any,
        provider: str = "unknown",
        model: str = "unknown",
        response: Any = None,
        error: Optional[Exception] = None,
    ) -> None:
        """Log API call details to file.

        Args:
            call_type: Type of API call (request, response, error)
            request: Request data
            provider: API provider name
            model: Model name
            response: Response data (for response logs)
            error: Error information (for error logs)
        """
        if not self.current_turn_dir:
            logger.warning("Cannot log API call: current_turn_dir not set")
            return

        try:
            # Create a timestamp for the log file
            timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")

            # Create filename based on log type
            filename = f"api_call_{timestamp}_{call_type}.json"
            filepath = os.path.join(self.current_turn_dir, filename)

            # Sanitize data before logging
            sanitized_request = self.sanitize_log_data(request)
            sanitized_response = self.sanitize_log_data(response) if response is not None else None

            # Prepare log data
            log_data = {
                "timestamp": timestamp,
                "provider": provider,
                "model": model,
                "type": call_type,
                "request": sanitized_request,
            }

            if sanitized_response is not None:
                log_data["response"] = sanitized_response
            if error is not None:
                log_data["error"] = str(error)

            # Write to file
            with open(filepath, "w") as f:
                json.dump(log_data, f, indent=2, default=str)

            logger.info(f"Logged API {call_type} to {filepath}")

        except Exception as e:
            logger.error(f"Error logging API call: {str(e)}")
