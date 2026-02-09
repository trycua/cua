"""MLX LVM client implementation."""

import base64
import io
import logging
import math
import os
import re
import tempfile
from typing import Any, Dict, List, Optional, Tuple, cast

import mlx.core as mx
from mlx_vlm import generate, load
from mlx_vlm.prompt_utils import apply_chat_template
from mlx_vlm.utils import load_config
from PIL import Image
from transformers.tokenization_utils import PreTrainedTokenizer

from .base import BaseUITarsClient

logger = logging.getLogger(__name__)

# Constants for smart_resize
IMAGE_FACTOR = 28
MIN_PIXELS = 100 * 28 * 28
MAX_PIXELS = 16384 * 28 * 28
MAX_RATIO = 200

def round_by_factor(number: float, factor: int) -> int:
    """Returns the closest integer to 'number' that is divisible by 'factor'."""
    return round(number / factor) * factor

def ceil_by_factor(number: float, factor: int) -> int:
    """Returns the smallest integer greater than or equal to 'number' that is divisible by 'factor'."""
    return math.ceil(number / factor) * factor

def floor_by_factor(number: float, factor: int) -> int:
    """Returns the largest integer less than or equal to 'number' that is divisible by 'factor'."""
    return math.floor(number / factor) * factor

def smart_resize(
    height: int, width: int, factor: int = IMAGE_FACTOR, min_pixels: int = MIN_PIXELS, max_pixels: int = MAX_PIXELS
) -> tuple[int, int]:
    """
    Rescales the image so that the following conditions are met:

    1. Both dimensions (height and width) are divisible by 'factor'.
    2. The total number of pixels is within the range ['min_pixels', 'max_pixels'].
    3. The aspect ratio of the image is maintained as closely as possible.
    """
    if max(height, width) / min(height, width) > MAX_RATIO:
        raise ValueError(
            f"absolute aspect ratio must be smaller than {MAX_RATIO}, got {max(height, width) / min(height, width)}"
        )
    h_bar = max(factor, round_by_factor(height, factor))
    w_bar = max(factor, round_by_factor(width, factor))
    if h_bar * w_bar > max_pixels:
        beta = math.sqrt((height * width) / max_pixels)
        h_bar = floor_by_factor(height / beta, factor)
        w_bar = floor_by_factor(width / beta, factor)
    elif h_bar * w_bar < min_pixels:
        beta = math.sqrt(min_pixels / (height * width))
        h_bar = ceil_by_factor(height * beta, factor)
        w_bar = ceil_by_factor(width * beta, factor)
    return h_bar, w_bar

class MLXVLMUITarsClient(BaseUITarsClient):
    """MLX LVM client implementation class."""

    def __init__(
        self, 
        model: str = "mlx-community/UI-TARS-1.5-7B-4bit"
    ):
        """Initialize MLX LVM client.

        Args:
            model: Model name or path (defaults to mlx-community/UI-TARS-1.5-7B-4bit)
        """
        # Load model and processor
        model_obj, processor = load(
            model, 
            processor_kwargs={"min_pixels": MIN_PIXELS, "max_pixels": MAX_PIXELS}
        )
        self.config = load_config(model)
        self.model = model_obj
        self.processor = processor
        self.model_name = model

    def _process_coordinates(self, text: str, original_size: Tuple[int, int], model_size: Tuple[int, int]) -> str:
        """Process coordinates in box tokens based on image resizing using smart_resize approach.
        
        Args:
            text: Text containing box tokens
            original_size: Original image size (width, height)
            model_size: Model processed image size (width, height)
            
        Returns:
            Text with processed coordinates
        """
        # Find all box tokens
        box_pattern = r"<\|box_start\|>\((\d+),\s*(\d+)\)<\|box_end\|>"
        
        def process_coords(match):
            model_x, model_y = int(match.group(1)), int(match.group(2))
            # Scale coordinates from model space to original image space
            # Both original_size and model_size are in (width, height) format
            new_x = int(model_x * original_size[0] / model_size[0])  # Width
            new_y = int(model_y * original_size[1] / model_size[1])  # Height
            return f"<|box_start|>({new_x},{new_y})<|box_end|>"
        
        return re.sub(box_pattern, process_coords, text)

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
        # Ensure the system message is included
        if not any(msg.get("role") == "system" for msg in messages):
            messages = [{"role": "system", "content": system}] + messages
        
        # Create a deep copy of messages to avoid modifying the original
        processed_messages = messages.copy()
        
        # Extract images and process messages
        images = []
        original_sizes = {}  # Track original sizes of images for coordinate mapping
        model_sizes = {}  # Track model processed sizes
        image_index = 0
        
        for msg_idx, msg in enumerate(messages):
            content = msg.get("content", [])
            if not isinstance(content, list):
                continue
                
            # Create a copy of the content list to modify
            processed_content = []
            
            for item_idx, item in enumerate(content):
                if item.get("type") == "image_url":
                    image_url = item.get("image_url", {}).get("url", "")
                    pil_image = None
                    
                    if image_url.startswith("data:image/"):
                        # Extract base64 data
                        base64_data = image_url.split(',')[1]
                        # Convert base64 to PIL Image
                        image_data = base64.b64decode(base64_data)
                        pil_image = Image.open(io.BytesIO(image_data))
                    else:
                        # Handle file path or URL
                        pil_image = Image.open(image_url)
                    
                    # Store original image size for coordinate mapping
                    original_size = pil_image.size
                    original_sizes[image_index] = original_size
                    
                    # Use smart_resize to determine model size
                    # Note: smart_resize expects (height, width) but PIL gives (width, height)
                    height, width = original_size[1], original_size[0]
                    new_height, new_width = smart_resize(height, width)
                    # Store model size in (width, height) format for consistent coordinate processing
                    model_sizes[image_index] = (new_width, new_height)
                    
                    # Resize the image using the calculated dimensions from smart_resize
                    resized_image = pil_image.resize((new_width, new_height))
                    images.append(resized_image)
                    image_index += 1
                
                # Copy items to processed content list
                processed_content.append(item.copy())
            
            # Update the processed message content
            processed_messages[msg_idx] = msg.copy()
            processed_messages[msg_idx]["content"] = processed_content
        
        logger.info(f"resized {len(images)} from {original_sizes[0]} to {model_sizes[0]}")
        
        # Process user text input with box coordinates after image processing
        # Swap original_size and model_size arguments for inverse transformation
        for msg_idx, msg in enumerate(processed_messages):
            if msg.get("role") == "user" and isinstance(msg.get("content"), str):
                if "<|box_start|>" in msg.get("content") and original_sizes and model_sizes and 0 in original_sizes and 0 in model_sizes:
                    orig_size = original_sizes[0]
                    model_size = model_sizes[0]
                    # Swap arguments to perform inverse transformation for user input
                    processed_messages[msg_idx]["content"] = self._process_coordinates(msg["content"], model_size, orig_size)
        
        try:
            # Format prompt according to model requirements using the processor directly
            prompt = self.processor.apply_chat_template(
                processed_messages,
                tokenize=False,
                add_generation_prompt=True
            )
            tokenizer = cast(PreTrainedTokenizer, self.processor)
            
            print("generating response...")
            
            # Generate response
            text_content, usage = generate(
                self.model, 
                tokenizer, 
                str(prompt), 
                images, 
                verbose=False,
                max_tokens=max_tokens
            )
            
            from pprint import pprint
            print("DEBUG - AGENT GENERATION --------")
            pprint(text_content)
            print("DEBUG - AGENT GENERATION --------")
        except Exception as e:
            logger.error(f"Error generating response: {str(e)}")
            return {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": f"Error generating response: {str(e)}"
                        },
                        "finish_reason": "error"
                    }
                ],
                "model": self.model_name,
                "error": str(e)
            }
        
        # Process coordinates in the response back to original image space
        if original_sizes and model_sizes and 0 in original_sizes and 0 in model_sizes:
            # Get original image size and model size (using the first image)
            orig_size = original_sizes[0]
            model_size = model_sizes[0]
            
            # Check if output contains box tokens that need processing
            if "<|box_start|>" in text_content:
                # Process coordinates from model space back to original image space
                text_content = self._process_coordinates(text_content, orig_size, model_size)
        
        # Format response to match OpenAI format
        response = {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": text_content
                    },
                    "finish_reason": "stop"
                }
            ],
            "model": self.model_name,
            "usage": usage
        }
        
        return response
