import base64
import re
from io import BytesIO
from typing import Any, Dict, List

try:
    import blobfile as _  # assert blobfile is installed
    import torch  # type: ignore
    from PIL import Image  # type: ignore
    from transformers import (  # type: ignore
        AutoImageProcessor,
        AutoModel,
        AutoTokenizer,
    )

    OPENCUA_AVAILABLE = True
except Exception:
    OPENCUA_AVAILABLE = False


class OpenCUAModel:
    """OpenCUA model handler using AutoTokenizer, AutoModel and AutoImageProcessor."""

    def __init__(
        self, model_name: str, device: str = "auto", trust_remote_code: bool = False
    ) -> None:
        if not OPENCUA_AVAILABLE:
            raise ImportError(
                'OpenCUA requirements not found. Install with: pip install "cua-agent[opencua-hf]"'
            )
        self.model_name = model_name
        self.device = device
        self.model = None
        self.tokenizer = None
        self.image_processor = None
        self.trust_remote_code = trust_remote_code
        self._load()

    def _load(self) -> None:
        self.tokenizer = AutoTokenizer.from_pretrained(
            self.model_name, trust_remote_code=self.trust_remote_code
        )
        self.model = AutoModel.from_pretrained(
            self.model_name,
            torch_dtype="auto",
            device_map=self.device,
            trust_remote_code=self.trust_remote_code,
            attn_implementation="sdpa",
        )
        self.image_processor = AutoImageProcessor.from_pretrained(
            self.model_name, trust_remote_code=self.trust_remote_code
        )

    @staticmethod
    def _extract_last_image_b64(messages: List[Dict[str, Any]]) -> str:
        # Expect HF-format messages with content items type: "image" with data URL
        for msg in reversed(messages):
            for item in reversed(msg.get("content", [])):
                if isinstance(item, dict) and item.get("type") == "image":
                    url = item.get("image", "")
                    if isinstance(url, str) and url.startswith("data:image/"):
                        return url.split(",", 1)[1]
        return ""

    def generate(self, messages: List[Dict[str, Any]], max_new_tokens: int = 512) -> str:
        assert (
            self.model is not None
            and self.tokenizer is not None
            and self.image_processor is not None
        )

        # Tokenize text side using chat template
        input_ids = self.tokenizer.apply_chat_template(
            messages, tokenize=True, add_generation_prompt=True
        )
        input_ids = torch.tensor([input_ids]).to(self.model.device)

        # Prepare image inputs from last data URL image
        image_b64 = self._extract_last_image_b64(messages)
        pixel_values = None
        grid_thws = None
        if image_b64:
            image = Image.open(BytesIO(base64.b64decode(image_b64))).convert("RGB")
            image_info = self.image_processor.preprocess(images=[image])
            pixel_values = torch.tensor(image_info["pixel_values"]).to(
                dtype=torch.bfloat16, device=self.model.device
            )
            grid_thws = (
                torch.tensor(image_info["image_grid_thw"])
                if "image_grid_thw" in image_info
                else None
            )

        gen_kwargs: Dict[str, Any] = {
            "max_new_tokens": max_new_tokens,
            "temperature": 0,
        }
        if pixel_values is not None:
            gen_kwargs["pixel_values"] = pixel_values
        if grid_thws is not None:
            gen_kwargs["grid_thws"] = grid_thws

        with torch.no_grad():
            generated_ids = self.model.generate(
                input_ids,
                **gen_kwargs,
            )

        # Remove prompt tokens
        prompt_len = input_ids.shape[1]
        generated_ids = generated_ids[:, prompt_len:]
        output_text = self.tokenizer.batch_decode(
            generated_ids, skip_special_tokens=True, clean_up_tokenization_spaces=False
        )[0]
        return output_text
