"""
GTA1 model implementation for benchmarking.
"""

import gc
import re
from typing import Optional, Tuple

import torch
from PIL import Image
from qwen_vl_utils import process_vision_info, smart_resize
from transformers import AutoProcessor, Qwen2_5_VLForConditionalGeneration

from .base import ModelProtocol


class GTA1Model:
    """Ground truth GTA1 model implementation."""

    def __init__(self, model_path: str = "HelloKKMe/GTA1-7B"):
        self.model_path = model_path
        self.model = None
        self.processor = None
        self.max_new_tokens = 32

        self.system_prompt = """
You are an expert UI element locator. Given a GUI image and a user's element description, provide the coordinates of the specified element as a single (x,y) point. The image resolution is height {height} and width {width}. For elements with area, return the center point.

Output the coordinate pair exactly:
(x,y)
""".strip()

    @property
    def model_name(self) -> str:
        """Return the name of the model."""
        return f"GTA1-{self.model_path.split('/')[-1]}"

    async def load_model(self) -> None:
        """Load the model into memory."""
        if self.model is None:
            print(f"Loading GTA1 model: {self.model_path}")
            self.model = Qwen2_5_VLForConditionalGeneration.from_pretrained(
                self.model_path, torch_dtype=torch.bfloat16, device_map="auto"
            )
            self.processor = AutoProcessor.from_pretrained(
                self.model_path, min_pixels=3136, max_pixels=4096 * 2160
            )
            print("GTA1 model loaded successfully")

    async def unload_model(self) -> None:
        """Unload the model from memory."""
        if self.model is not None:
            print("Unloading GTA1 model from GPU...")
            del self.model
            del self.processor
            self.model = None
            self.processor = None
            gc.collect()
            if torch.cuda.is_available():
                torch.cuda.empty_cache()
            print("GTA1 model unloaded")

    def _extract_coordinates(self, raw_string: str) -> Tuple[int, int]:
        """Extract coordinates from model output."""
        try:
            matches = re.findall(r"\((-?\d*\.?\d+),\s*(-?\d*\.?\d+)\)", raw_string)
            return tuple(map(int, map(float, matches[0])))  # type: ignore
        except:
            return (0, 0)

    async def predict_click(
        self, image: Image.Image, instruction: str
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates for the given image and instruction.

        Args:
            image: PIL Image to analyze
            instruction: Text instruction describing what to click

        Returns:
            Tuple of (x, y) coordinates or None if prediction fails
        """
        if self.model is None or self.processor is None:
            await self.load_model()

        assert self.processor is not None
        assert self.model is not None

        try:
            width, height = image.width, image.height

            # Resize image according to processor requirements
            resized_height, resized_width = smart_resize(
                image.height,
                image.width,
                factor=self.processor.image_processor.patch_size
                * self.processor.image_processor.merge_size,
                min_pixels=self.processor.image_processor.min_pixels,
                max_pixels=self.processor.image_processor.max_pixels,
            )
            resized_image = image.resize((resized_width, resized_height))
            scale_x, scale_y = width / resized_width, height / resized_height

            # Prepare messages
            system_message = {
                "role": "system",
                "content": self.system_prompt.format(height=resized_height, width=resized_width),
            }

            user_message = {
                "role": "user",
                "content": [
                    {"type": "image", "image": resized_image},
                    {"type": "text", "text": instruction},
                ],
            }

            # Process inputs
            image_inputs, video_inputs = process_vision_info([system_message, user_message])  # type: ignore
            text = self.processor.apply_chat_template(
                [system_message, user_message], tokenize=False, add_generation_prompt=True
            )
            inputs = self.processor(
                text=[text],
                images=image_inputs,
                videos=video_inputs,
                padding=True,
                return_tensors="pt",
            )
            inputs = inputs.to(self.model.device)

            # Generate prediction
            output_ids = self.model.generate(
                **inputs,
                max_new_tokens=self.max_new_tokens,
                do_sample=False,
                temperature=1.0,
                use_cache=True,
            )
            generated_ids = [
                output_ids[len(input_ids) :]
                for input_ids, output_ids in zip(inputs.input_ids, output_ids)
            ]
            output_text = self.processor.batch_decode(
                generated_ids, skip_special_tokens=True, clean_up_tokenization_spaces=True
            )[0]

            # Extract and rescale coordinates
            pred_x, pred_y = self._extract_coordinates(output_text)
            pred_x = int(pred_x * scale_x)
            pred_y = int(pred_y * scale_y)

            return (pred_x, pred_y)

        except Exception as e:
            print(f"Error in GTA1 prediction: {e}")
            return None
