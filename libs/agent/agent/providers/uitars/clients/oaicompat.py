"""OpenAI-compatible client implementation."""

import os
import logging
from typing import Dict, List, Optional, Any
import aiohttp
import re
from .base import BaseUITarsClient

logger = logging.getLogger(__name__)


# OpenAI-compatible client for the UI_Tars
class OAICompatClient(BaseUITarsClient):
    """OpenAI-compatible API client implementation.

    This client can be used with any service that implements the OpenAI API protocol, including:
    - Huggingface Text Generation Interface endpoints
    - vLLM
    - LM Studio
    - LocalAI
    - Ollama (with OpenAI compatibility)
    - Text Generation WebUI
    - Any other service with OpenAI API compatibility
    """

    def __init__(
        self,
        api_key: Optional[str] = None,
        model: str = "Qwen2.5-VL-7B-Instruct",
        provider_base_url: Optional[str] = "http://localhost:8000/v1",
        max_tokens: int = 4096,
        temperature: float = 0.0,
    ):
        """Initialize the OpenAI-compatible client.

        Args:
            api_key: Not used for local endpoints, usually set to "EMPTY"
            model: Model name to use
            provider_base_url: API base URL. Typically in the format "http://localhost:PORT/v1"
                Examples:
                - vLLM: "http://localhost:8000/v1"
                - LM Studio: "http://localhost:1234/v1"
                - LocalAI: "http://localhost:8080/v1"
                - Ollama: "http://localhost:11434/v1"
            max_tokens: Maximum tokens to generate
            temperature: Generation temperature
        """
        super().__init__(api_key=api_key or "EMPTY", model=model)
        self.api_key = api_key or "EMPTY" # Local endpoints typically don't require an API key
        self.model = model
        self.provider_base_url = (
            provider_base_url or "http://localhost:8000/v1"
        )  # Use default if None
        self.max_tokens = max_tokens
        self.temperature = temperature

    def _extract_base64_image(self, text: str) -> Optional[str]:
        """Extract base64 image data from an HTML img tag."""
        pattern = r'data:image/[^;]+;base64,([^"]+)'
        match = re.search(pattern, text)
        return match.group(1) if match else None

    def _get_loggable_messages(self, messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Create a loggable version of messages with image data truncated."""
        loggable_messages = []
        for msg in messages:
            if isinstance(msg.get("content"), list):
                new_content = []
                for content in msg["content"]:
                    if content.get("type") == "image":
                        new_content.append(
                            {"type": "image", "image_url": {"url": "[BASE64_IMAGE_DATA]"}}
                        )
                    else:
                        new_content.append(content)
                loggable_messages.append({"role": msg["role"], "content": new_content})
            else:
                loggable_messages.append(msg)
        return loggable_messages

    async def run_interleaved(
        self, messages: List[Dict[str, Any]], system: str, max_tokens: Optional[int] = None
    ) -> Dict[str, Any]:
        """Run interleaved chat completion.

        Args:
            messages: List of message dicts
            system: System prompt
            max_tokens: Optional max tokens override

        Returns:
            Response dict
        """
        headers = {"Content-Type": "application/json", "Authorization": f"Bearer {self.api_key}"}

        final_messages = [
            {
                "role": "system", 
                "content": [
                    { "type": "text", "text": system }
                ]
            }
        ]
        
        # Process messages
        for item in messages:
            if isinstance(item, dict):
                if isinstance(item["content"], list):
                    # Content is already in the correct format
                    final_messages.append(item)
                else:
                    # Single string content, check for image
                    base64_img = self._extract_base64_image(item["content"])
                    if base64_img:
                        message = {
                            "role": item["role"],
                            "content": [
                                {
                                    "type": "image_url",
                                    "image_url": {"url": f"data:image/jpeg;base64,{base64_img}"},
                                }
                            ],
                        }
                    else:
                        message = {
                            "role": item["role"],
                            "content": [{"type": "text", "text": item["content"]}],
                        }
                    final_messages.append(message)
            else:
                # String content, check for image
                base64_img = self._extract_base64_image(item)
                if base64_img:
                    message = {
                        "role": "user",
                        "content": [
                            {
                                "type": "image_url",
                                "image_url": {"url": f"data:image/jpeg;base64,{base64_img}"},
                            }
                        ],
                    }
                else:
                    message = {"role": "user", "content": [{"type": "text", "text": item}]}
                final_messages.append(message)

        payload = {
            "model": self.model, 
            "messages": final_messages, 
            "max_tokens": max_tokens or self.max_tokens,
            "temperature": self.temperature,
            "top_p": 0.7,
        }

        try:
            async with aiohttp.ClientSession() as session:
                # Use default base URL if none provided
                base_url = self.provider_base_url or "http://localhost:8000/v1"

                # Check if the base URL already includes the chat/completions endpoint
                
                endpoint_url = base_url
                if not endpoint_url.endswith("/chat/completions"):
                    # If URL is RunPod format, make it OpenAI compatible
                    if endpoint_url.startswith("https://api.runpod.ai/v2/"):
                        # Extract RunPod endpoint ID
                        parts = endpoint_url.split("/")
                        if len(parts) >= 5:
                            runpod_id = parts[4]
                            endpoint_url = f"https://api.runpod.ai/v2/{runpod_id}/openai/v1/chat/completions"
                    # If the URL ends with /v1, append /chat/completions
                    elif endpoint_url.endswith("/v1"):
                        endpoint_url = f"{endpoint_url}/chat/completions"
                    # If the URL doesn't end with /v1, make sure it has a proper structure
                    elif not endpoint_url.endswith("/"):
                        endpoint_url = f"{endpoint_url}/chat/completions"
                    else:
                        endpoint_url = f"{endpoint_url}chat/completions"

                # Log the endpoint URL for debugging
                logger.debug(f"Using endpoint URL: {endpoint_url}")

                async with session.post(endpoint_url, headers=headers, json=payload) as response:
                    # Log the status and content type
                    logger.debug(f"Status: {response.status}")
                    logger.debug(f"Content-Type: {response.headers.get('Content-Type')}")
                    
                    # Get the raw text of the response
                    response_text = await response.text()
                    logger.debug(f"Response content: {response_text}")
                    
                    # if 503, then the endpoint is still warming up
                    if response.status == 503:
                        logger.error(f"Endpoint is still warming up, please try again later")
                        raise Exception(f"Endpoint is still warming up: {response_text}")
                    
                    # Try to parse as JSON if the content type is appropriate
                    if "application/json" in response.headers.get('Content-Type', ''):
                        response_json = await response.json()
                    else:
                        raise Exception(f"Response is not JSON format")

                    if response.status != 200:
                        logger.error(f"Error in API call: {response_text}")
                        raise Exception(f"API error: {response_text}")
                    
                    return response_json

        except Exception as e:
            logger.error(f"Error in API call: {str(e)}")
            raise
