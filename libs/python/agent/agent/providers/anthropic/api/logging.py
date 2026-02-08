"""API logging functionality."""

import json
import logging
from datetime import datetime
from pathlib import Path
from typing import Any

import httpx

logger = logging.getLogger(__name__)

def _filter_base64_images(content: Any) -> Any:
    """Filter out base64 image data from content.
    
    Args:
        content: Content to filter
        
    Returns:
        Filtered content with base64 data replaced by placeholder
    """
    if isinstance(content, dict):
        filtered = {}
        for key, value in content.items():
            if (
                isinstance(value, dict) 
                and value.get("type") == "image" 
                and value.get("source", {}).get("type") == "base64"
            ):
                # Replace base64 data with placeholder
                filtered[key] = {
                    **value,
                    "source": {
                        **value["source"],
                        "data": "<base64_image_data>"
                    }
                }
            else:
                filtered[key] = _filter_base64_images(value)
        return filtered
    elif isinstance(content, list):
        return [_filter_base64_images(item) for item in content]
    return content

def log_api_interaction(
    request: httpx.Request | None,
    response: httpx.Response | object | None,
    error: Exception | None,
    log_dir: Path = Path("/tmp/claude_logs")
) -> None:
    """Log API request, response, and any errors in a structured way.
    
    Args:
        request: The HTTP request if available
        response: The HTTP response or response object
        error: Any error that occurred
        log_dir: Directory to store log files
    """
    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")
    
    # Helper function to safely decode JSON content
    def safe_json_decode(content):
        if not content:
            return None
        try:
            if isinstance(content, bytes):
                return json.loads(content.decode())
            elif isinstance(content, str):
                return json.loads(content)
            elif isinstance(content, dict):
                return content
            return None
        except json.JSONDecodeError:
            return {"error": "Could not decode JSON", "raw": str(content)}

    # Process request content
    request_content = None
    if request and request.content:
        request_content = safe_json_decode(request.content)
        request_content = _filter_base64_images(request_content)

    # Process response content
    response_content = None
    if response:
        if isinstance(response, httpx.Response):
            try:
                response_content = response.json()
            except json.JSONDecodeError:
                response_content = {"error": "Could not decode JSON", "raw": response.text}
        else:
            response_content = safe_json_decode(response)
        response_content = _filter_base64_images(response_content)

    log_entry = {
        "timestamp": timestamp,
        "request": {
            "method": request.method if request else None,
            "url": str(request.url) if request else None,
            "headers": dict(request.headers) if request else None,
            "content": request_content,
        } if request else None,
        "response": {
            "status_code": response.status_code if isinstance(response, httpx.Response) else None,
            "headers": dict(response.headers) if isinstance(response, httpx.Response) else None,
            "content": response_content,
        } if response else None,
        "error": {
            "type": type(error).__name__ if error else None,
            "message": str(error) if error else None,
        } if error else None
    }
    
    # Log to file with timestamp in filename
    log_dir.mkdir(exist_ok=True)
    log_file = log_dir / f"claude_api_{timestamp.replace(' ', '_').replace(':', '-')}.json"
    
    with open(log_file, 'w') as f:
        json.dump(log_entry, f, indent=2)
    
    # Also log a summary to the console
    if error:
        logger.error(f"API Error at {timestamp}: {error}")
    else:
        logger.info(
            f"API Call at {timestamp}: "
            f"{request.method if request else 'No request'} -> "
            f"{response.status_code if isinstance(response, httpx.Response) else 'No response'}"
        )
        
        # Log if there are any images in the content
        if response_content:
            image_count = count_images(response_content)
            if image_count > 0:
                logger.info(f"Response contains {image_count} images")

def count_images(content: dict | list | Any) -> int:
    """Count the number of images in the content.
    
    Args:
        content: Content to search for images
        
    Returns:
        Number of images found
    """
    if isinstance(content, dict):
        if content.get("type") == "image":
            return 1
        return sum(count_images(v) for v in content.values())
    elif isinstance(content, list):
        return sum(count_images(item) for item in content)
    return 0 