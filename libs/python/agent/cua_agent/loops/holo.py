"""
Holo 1.5 agent loop implementation for click prediction using litellm.acompletion.

Implements the Holo1.5 grounding behavior:
- Prompt asks for absolute pixel coordinates in JSON: {"action":"click_absolute","x":int,"y":int}
- Optionally resizes the image using Qwen2-VL smart_resize parameters (via transformers AutoProcessor)
- If resized, maps predicted coordinates back to the original screenshot resolution

Note: We do NOT manually load the model; acompletions (via HuggingFaceLocalAdapter)
will handle loading based on the provided model name.
"""

from __future__ import annotations

import base64
import json
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

import litellm
from PIL import Image

from ..decorators import register_agent
from ..types import AgentCapability
from .base import AsyncAgentConfig


def _strip_hf_prefix(model: str) -> str:
    """Strip provider prefixes like 'huggingface-local/' from model names for HF processor load."""
    if "/" in model and model.lower().startswith("huggingface-local/"):
        return model.split("/", 1)[1]
    return model


def _maybe_smart_resize(image: Image.Image, model: str) -> Tuple[Image.Image, Tuple[int, int]]:
    """
    Try to compute Qwen2-VL smart_resize output size using transformers AutoProcessor.

    Returns (processed_image, (orig_w, orig_h)). If transformers or processor unavailable,
    returns the original image and size without resizing.
    """
    orig_w, orig_h = image.size
    try:
        # Import lazily to avoid hard dependency if not installed
        from transformers import AutoProcessor  # type: ignore
        from transformers.models.qwen2_vl.image_processing_qwen2_vl import (  # type: ignore
            smart_resize,
        )

        processor_name = _strip_hf_prefix(model)
        processor = AutoProcessor.from_pretrained(processor_name)
        image_processor = getattr(processor, "image_processor", None)
        if image_processor is None:
            return image, (orig_w, orig_h)

        factor = getattr(image_processor, "patch_size", 14) * getattr(
            image_processor, "merge_size", 1
        )
        min_pixels = getattr(image_processor, "min_pixels", 256 * 256)
        max_pixels = getattr(image_processor, "max_pixels", 1536 * 1536)

        resized_h, resized_w = smart_resize(
            orig_h,
            orig_w,
            factor=factor,
            min_pixels=min_pixels,
            max_pixels=max_pixels,
        )

        if (resized_w, resized_h) == (orig_w, orig_h):
            return image, (orig_w, orig_h)

        processed = image.resize((resized_w, resized_h), resample=Image.Resampling.LANCZOS)
        return processed, (orig_w, orig_h)
    except Exception:
        # If any failure (no transformers, processor load error), fall back to original
        return image, (orig_w, orig_h)


def _build_holo_prompt(instruction: str) -> str:
    """Construct the Holo1.5 grounding prompt."""
    # Keep it close to the cookbook while avoiding heavy schema generation
    schema_hint = '{"action": "click_absolute", "x": <int>, "y": <int>}'
    return (
        "Localize an element on the GUI image according to the provided target and output a click position. "
        f"You must output a valid JSON following the format: {schema_hint} "
        f"Your target is: {instruction}"
    )


def _parse_click_json(output_text: str) -> Optional[Tuple[int, int]]:
    """
    Parse JSON from model output and extract x, y ints.
    Tries to find the first JSON object substring if extra text is present.
    """
    try:
        # Fast path: direct JSON
        data = json.loads(output_text)
    except Exception:
        # Try to locate a JSON object within the text
        start = output_text.find("{")
        end = output_text.rfind("}")
        if start == -1 or end == -1 or end <= start:
            return None
        try:
            data = json.loads(output_text[start : end + 1])
        except Exception:
            return None

    try:
        x = int(data.get("x"))
        y = int(data.get("y"))
        return x, y
    except Exception:
        return None


@register_agent(models=r"(?i).*(Holo1\.5|Hcompany/Holo1\.5).*")
class HoloConfig(AsyncAgentConfig):
    """Holo is a family of UI grounding models from H Company"""

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
        **kwargs,
    ) -> Dict[str, Any]:
        # Holo models are only trained on UI localization tasks, not all-in-one agent
        raise NotImplementedError()

    async def predict_click(
        self,
        model: str,
        image_b64: str,
        instruction: str,
        **kwargs,
    ) -> Optional[Tuple[int, int]]:
        """
        Predict click coordinates using Holo1.5 via litellm.acompletion.

        - Optionally smart-resizes the image using Qwen2-VL rules if transformers are available
        - Prompts for JSON with absolute pixel coordinates
        - Parses x,y and maps back to original screenshot size if resized
        """
        try:
            img_bytes = base64.b64decode(image_b64)
            original_img = Image.open(BytesIO(img_bytes))
        except Exception:
            return None

        # Optional preprocessing
        processed_img, (orig_w, orig_h) = _maybe_smart_resize(original_img, model)

        # If we resized, send the resized image; otherwise send original
        img_to_send = processed_img
        buf = BytesIO()
        img_to_send.save(buf, format="PNG")
        processed_b64 = base64.b64encode(buf.getvalue()).decode("utf-8")

        prompt = _build_holo_prompt(instruction)

        messages = [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{processed_b64}"},
                    },
                    {"type": "text", "text": prompt},
                ],
            }
        ]

        api_kwargs = {
            "model": model,
            "messages": messages,
            # Deterministic, small output
            "max_tokens": kwargs.get("max_tokens", 256),
            "temperature": kwargs.get("temperature", 0.0),
        }

        response = await litellm.acompletion(**api_kwargs)
        output_text = (response.choices[0].message.content or "").strip()  # type: ignore

        coords = _parse_click_json(output_text)
        if coords is None:
            return None

        x, y = coords

        # Map back to original size if we resized
        proc_w, proc_h = img_to_send.size
        if (proc_w, proc_h) != (orig_w, orig_h):
            try:
                sx = orig_w / float(proc_w)
                sy = orig_h / float(proc_h)
                x = int(round(x * sx))
                y = int(round(y * sy))
            except Exception:
                # Fallback: clamp within original bounds
                pass

        # Clamp to original image bounds
        x = max(0, min(orig_w - 1, x))
        y = max(0, min(orig_h - 1, y))
        return x, y

    def get_capabilities(self) -> List[AgentCapability]:
        return ["click"]
