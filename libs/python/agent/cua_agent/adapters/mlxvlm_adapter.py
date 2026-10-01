import asyncio
import base64
import functools
import io
import math
import re
import warnings
from concurrent.futures import ThreadPoolExecutor
from typing import Any, AsyncIterator, Dict, Iterator, List, Optional, Tuple, cast

from litellm import acompletion, completion
from litellm.llms.custom_llm import CustomLLM
from litellm.types.utils import GenericStreamingChunk, ModelResponse
from PIL import Image

# Try to import MLX dependencies
try:
    import mlx.core as mx
    from mlx_vlm import generate, load
    from mlx_vlm.prompt_utils import apply_chat_template
    from mlx_vlm.utils import load_config
    from transformers.tokenization_utils import PreTrainedTokenizer

    MLX_AVAILABLE = True
except ImportError:
    MLX_AVAILABLE = False

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
    height: int,
    width: int,
    factor: int = IMAGE_FACTOR,
    min_pixels: int = MIN_PIXELS,
    max_pixels: int = MAX_PIXELS,
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


class MLXVLMAdapter(CustomLLM):
    """MLX VLM Adapter for running vision-language models locally using MLX."""

    def __init__(self, **kwargs):
        """Initialize the adapter.

        Args:
            **kwargs: Additional arguments
        """
        super().__init__()

        self.models = {}  # Cache for loaded models
        self.processors = {}  # Cache for loaded processors
        self.configs = {}  # Cache for loaded configs
        self._executor = ThreadPoolExecutor(max_workers=1)  # Single thread pool

    def _load_model_and_processor(self, model_name: str):
        """Load model and processor if not already cached.

        Args:
            model_name: Name of the model to load

        Returns:
            Tuple of (model, processor, config)
        """
        if not MLX_AVAILABLE:
            raise ImportError("MLX VLM dependencies not available. Please install mlx-vlm.")

        if model_name not in self.models:
            # Load model and processor
            model_obj, processor = load(
                model_name, processor_kwargs={"min_pixels": MIN_PIXELS, "max_pixels": MAX_PIXELS}
            )
            config = load_config(model_name)

            # Cache them
            self.models[model_name] = model_obj
            self.processors[model_name] = processor
            self.configs[model_name] = config

        return self.models[model_name], self.processors[model_name], self.configs[model_name]

    def _process_coordinates(
        self, text: str, original_size: Tuple[int, int], model_size: Tuple[int, int]
    ) -> str:
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

    def _convert_messages(self, messages: List[Dict[str, Any]]) -> Tuple[
        List[Dict[str, Any]],
        List[Image.Image],
        Dict[int, Tuple[int, int]],
        Dict[int, Tuple[int, int]],
    ]:
        """Convert OpenAI format messages to MLX VLM format and extract images.

        Args:
            messages: Messages in OpenAI format

        Returns:
            Tuple of (processed_messages, images, original_sizes, model_sizes)
        """
        processed_messages = []
        images = []
        original_sizes = {}  # Track original sizes of images for coordinate mapping
        model_sizes = {}  # Track model processed sizes
        image_index = 0

        for message in messages:
            processed_message = {"role": message["role"], "content": []}

            content = message.get("content", [])
            if isinstance(content, str):
                # Simple text content
                processed_message["content"] = content
            elif isinstance(content, list):
                # Multi-modal content
                processed_content = []
                for item in content:
                    if item.get("type") == "text":
                        processed_content.append({"type": "text", "text": item.get("text", "")})
                    elif item.get("type") == "image_url":
                        image_url = item.get("image_url", {}).get("url", "")
                        pil_image = None

                        if image_url.startswith("data:image/"):
                            # Extract base64 data
                            base64_data = image_url.split(",")[1]
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

                        # Add image placeholder to content
                        processed_content.append({"type": "image"})

                        image_index += 1

                processed_message["content"] = processed_content

            processed_messages.append(processed_message)

        return processed_messages, images, original_sizes, model_sizes

    def _generate(self, **kwargs) -> str:
        """Generate response using the local MLX VLM model.

        Args:
            **kwargs: Keyword arguments containing messages and model info

        Returns:
            Generated text response
        """
        messages = kwargs.get("messages", [])
        model_name = kwargs.get("model", "mlx-community/UI-TARS-1.5-7B-4bit")
        max_tokens = kwargs.get("max_tokens", 128)

        # Warn about ignored kwargs
        ignored_kwargs = set(kwargs.keys()) - {"messages", "model", "max_tokens"}
        if ignored_kwargs:
            warnings.warn(f"Ignoring unsupported kwargs: {ignored_kwargs}")

        # Load model and processor
        model, processor, config = self._load_model_and_processor(model_name)

        # Convert messages and extract images
        processed_messages, images, original_sizes, model_sizes = self._convert_messages(messages)

        # Process user text input with box coordinates after image processing
        # Swap original_size and model_size arguments for inverse transformation
        for msg_idx, msg in enumerate(processed_messages):
            if msg.get("role") == "user" and isinstance(msg.get("content"), str):
                content = msg.get("content", "")
                if (
                    "<|box_start|>" in content
                    and original_sizes
                    and model_sizes
                    and 0 in original_sizes
                    and 0 in model_sizes
                ):
                    orig_size = original_sizes[0]
                    model_size = model_sizes[0]
                    # Swap arguments to perform inverse transformation for user input
                    processed_messages[msg_idx]["content"] = self._process_coordinates(
                        content, model_size, orig_size
                    )

        try:
            # Format prompt according to model requirements using the processor directly
            prompt = processor.apply_chat_template(
                processed_messages, tokenize=False, add_generation_prompt=True, return_tensors="pt"
            )
            tokenizer = cast(PreTrainedTokenizer, processor)

            # Generate response
            text_content, usage = generate(
                model,
                tokenizer,
                str(prompt),
                images,  # type: ignore
                verbose=False,
                max_tokens=max_tokens,
            )

        except Exception as e:
            raise RuntimeError(f"Error generating response: {str(e)}") from e

        # Process coordinates in the response back to original image space
        if original_sizes and model_sizes and 0 in original_sizes and 0 in model_sizes:
            # Get original image size and model size (using the first image)
            orig_size = original_sizes[0]
            model_size = model_sizes[0]

            # Check if output contains box tokens that need processing
            if "<|box_start|>" in text_content:
                # Process coordinates from model space back to original image space
                text_content = self._process_coordinates(text_content, orig_size, model_size)

        return text_content

    def completion(self, *args, **kwargs) -> ModelResponse:
        """Synchronous completion method.

        Returns:
            ModelResponse with generated text
        """
        generated_text = self._generate(**kwargs)

        result = completion(
            model=f"mlx/{kwargs.get('model', 'mlx-community/UI-TARS-1.5-7B-4bit')}",
            mock_response=generated_text,
        )
        return cast(ModelResponse, result)

    async def acompletion(self, *args, **kwargs) -> ModelResponse:
        """Asynchronous completion method.

        Returns:
            ModelResponse with generated text
        """
        # Run _generate in thread pool to avoid blocking
        loop = asyncio.get_event_loop()
        generated_text = await loop.run_in_executor(
            self._executor, functools.partial(self._generate, **kwargs)
        )

        result = await acompletion(
            model=f"mlx/{kwargs.get('model', 'mlx-community/UI-TARS-1.5-7B-4bit')}",
            mock_response=generated_text,
        )
        return cast(ModelResponse, result)

    def streaming(self, *args, **kwargs) -> Iterator[GenericStreamingChunk]:
        """Synchronous streaming method.

        Returns:
            Iterator of GenericStreamingChunk
        """
        generated_text = self._generate(**kwargs)

        generic_streaming_chunk: GenericStreamingChunk = {
            "finish_reason": "stop",
            "index": 0,
            "is_finished": True,
            "text": generated_text,
            "tool_use": None,
            "usage": {"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0},
        }

        yield generic_streaming_chunk

    async def astreaming(self, *args, **kwargs) -> AsyncIterator[GenericStreamingChunk]:
        """Asynchronous streaming method.

        Returns:
            AsyncIterator of GenericStreamingChunk
        """
        # Run _generate in thread pool to avoid blocking
        loop = asyncio.get_event_loop()
        generated_text = await loop.run_in_executor(
            self._executor, functools.partial(self._generate, **kwargs)
        )

        generic_streaming_chunk: GenericStreamingChunk = {
            "finish_reason": "stop",
            "index": 0,
            "is_finished": True,
            "text": generated_text,
            "tool_use": None,
            "usage": {"completion_tokens": 0, "prompt_tokens": 0, "total_tokens": 0},
        }

        yield generic_streaming_chunk
