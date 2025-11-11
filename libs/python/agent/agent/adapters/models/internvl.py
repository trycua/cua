from __future__ import annotations

from typing import Any, Dict, List, Optional

# Hugging Face imports are local to avoid hard dependency at module import
try:
    import base64  # type: ignore
    from io import BytesIO  # type: ignore

    # Attempt to import InternVL's model dependencies
    import einops as _  # type: ignore
    import requests  # type: ignore
    import timm as _  # type: ignore
    import torch  # type: ignore
    import torchvision.transforms as T  # type: ignore
    from PIL import Image  # type: ignore
    from torchvision.transforms.functional import InterpolationMode  # type: ignore
    from transformers import AutoModel, AutoTokenizer  # type: ignore

    HF_AVAILABLE = True
except Exception:
    HF_AVAILABLE = False


class InternVLModel:
    """Generic Hugging Face vision-language model handler.
    Uses InternVL's native `model.chat()` interface with `AutoTokenizer`.
    Provides preprocessing to support multi-turn conversations with multiple images.
    """

    def __init__(
        self, model_name: str, device: str = "auto", trust_remote_code: bool = False
    ) -> None:
        if not HF_AVAILABLE:
            raise ImportError(
                'InternVL dependencies not found. Install with: pip install "cua-agent[internvl-hf]"'
            )
        self.model_name = model_name
        self.device = device
        self.model = None
        self.tokenizer = None
        self.trust_remote_code = trust_remote_code
        self._load()

    def _load(self) -> None:
        # Load model
        self.model = AutoModel.from_pretrained(
            self.model_name,
            torch_dtype=torch.bfloat16,
            low_cpu_mem_usage=True,
            use_flash_attn=True,
            device_map=self.device,
            trust_remote_code=self.trust_remote_code,
        ).eval()
        # Load tokenizer (InternVL requires trust_remote_code=True and often use_fast=False)
        self.tokenizer = AutoTokenizer.from_pretrained(
            self.model_name,
            trust_remote_code=self.trust_remote_code,
            use_fast=False,
        )

    # ---- Image preprocessing utilities adapted from InternVL docs ----
    IMAGENET_MEAN = (0.485, 0.456, 0.406)
    IMAGENET_STD = (0.229, 0.224, 0.225)

    def _build_transform(self, input_size: int) -> T.Compose:
        MEAN, STD = self.IMAGENET_MEAN, self.IMAGENET_STD
        transform = T.Compose(
            [
                T.Lambda(lambda img: img.convert("RGB") if img.mode != "RGB" else img),
                T.Resize((input_size, input_size), interpolation=InterpolationMode.BICUBIC),
                T.ToTensor(),
                T.Normalize(mean=MEAN, std=STD),
            ]
        )
        return transform

    def _find_closest_aspect_ratio(
        self,
        aspect_ratio: float,
        target_ratios: List[tuple],
        width: int,
        height: int,
        image_size: int,
    ):
        best_ratio_diff = float("inf")
        best_ratio = (1, 1)
        area = width * height
        for ratio in target_ratios:
            target_aspect_ratio = ratio[0] / ratio[1]
            ratio_diff = abs(aspect_ratio - target_aspect_ratio)
            if ratio_diff < best_ratio_diff:
                best_ratio_diff = ratio_diff
                best_ratio = ratio
            elif ratio_diff == best_ratio_diff:
                if area > 0.5 * image_size * image_size * ratio[0] * ratio[1]:
                    best_ratio = ratio
        return best_ratio

    def _dynamic_preprocess(
        self,
        image: Image.Image,
        min_num: int = 1,
        max_num: int = 12,
        image_size: int = 448,
        use_thumbnail: bool = True,
    ) -> List[Image.Image]:
        orig_width, orig_height = image.size
        aspect_ratio = orig_width / orig_height

        target_ratios = set(
            (i, j)
            for n in range(min_num, max_num + 1)
            for i in range(1, n + 1)
            for j in range(1, n + 1)
            if i * j <= max_num and i * j >= min_num
        )
        target_ratios = sorted(target_ratios, key=lambda x: x[0] * x[1])

        target_aspect_ratio = self._find_closest_aspect_ratio(
            aspect_ratio, target_ratios, orig_width, orig_height, image_size
        )

        target_width = image_size * target_aspect_ratio[0]
        target_height = image_size * target_aspect_ratio[1]
        blocks = target_aspect_ratio[0] * target_aspect_ratio[1]

        resized_img = image.resize((target_width, target_height))
        processed_images: List[Image.Image] = []
        for i in range(blocks):
            box = (
                (i % (target_width // image_size)) * image_size,
                (i // (target_width // image_size)) * image_size,
                ((i % (target_width // image_size)) + 1) * image_size,
                ((i // (target_width // image_size)) + 1) * image_size,
            )
            split_img = resized_img.crop(box)
            processed_images.append(split_img)
        assert len(processed_images) == blocks
        if use_thumbnail and len(processed_images) != 1:
            thumbnail_img = image.resize((image_size, image_size))
            processed_images.append(thumbnail_img)
        return processed_images

    def _load_image_from_source(self, src: str) -> Image.Image:
        """Load PIL image from various sources: data URL, http(s), or local path."""
        if src.startswith("data:image/"):
            # data URL base64
            header, b64data = src.split(",", 1)
            img_bytes = base64.b64decode(b64data)
            return Image.open(BytesIO(img_bytes)).convert("RGB")
        if src.startswith("http://") or src.startswith("https://"):
            resp = requests.get(src, timeout=10)
            resp.raise_for_status()
            return Image.open(BytesIO(resp.content)).convert("RGB")
        # Assume local file path
        return Image.open(src).convert("RGB")

    def _images_to_pixel_values(
        self, images: List[Image.Image], input_size: int = 448, max_num: int = 12
    ):
        transform = self._build_transform(input_size=input_size)
        pixel_values_list = []
        num_patches_list: List[int] = []
        for img in images:
            tiles = self._dynamic_preprocess(
                img, image_size=input_size, use_thumbnail=True, max_num=max_num
            )
            pv = [transform(tile) for tile in tiles]
            pv = torch.stack(pv)
            num_patches_list.append(pv.shape[0])
            pixel_values_list.append(pv)
        if not pixel_values_list:
            return None, []
        pixel_values = torch.cat(pixel_values_list)
        return pixel_values, num_patches_list

    def generate(self, messages: List[Dict[str, Any]], max_new_tokens: int = 128) -> str:
        """Generate text for the given HF-format messages.
        messages: [{ role, content: [{type:'text'|'image', text|image}] }]

        This implementation constructs InternVL-compatible inputs and uses
        `model.chat(tokenizer, pixel_values, question, history=...)` to avoid
        relying on AutoProcessor (which fails for some tokenizers).
        """
        assert self.model is not None and self.tokenizer is not None

        # Build textual context and collect images and the final question
        context_lines: List[str] = []
        all_images: List[Image.Image] = []
        last_user_text_parts: List[str] = []

        for msg in messages:
            role = msg.get("role", "user")
            content = msg.get("content", [])
            if isinstance(content, str):
                content_items = [{"type": "text", "text": content}]
            else:
                content_items = content

            if role == "user":
                # Collect text and images
                parts_text: List[str] = []
                for item in content_items:
                    if item.get("type") == "text":
                        t = item.get("text", "")
                        if t:
                            parts_text.append(t)
                    elif item.get("type") == "image":
                        url = item.get("image", "")
                        if url:
                            try:
                                all_images.append(self._load_image_from_source(url))
                            except Exception:
                                # Ignore failed image loads but keep going
                                pass
                text = "\n".join(parts_text).strip()
                if text:
                    context_lines.append(f"User: {text}")
                # Track last user text separately for question
                last_user_text_parts = parts_text or last_user_text_parts
            elif role == "assistant":
                # Only keep text content for history
                parts_text = [
                    item.get("text", "") for item in content_items if item.get("type") == "text"
                ]
                text = "\n".join(parts_text).strip()
                if text:
                    context_lines.append(f"Assistant: {text}")

        # Prepare pixel values for all collected images (across turns)
        pixel_values = None
        num_patches_list: List[int] = []
        if all_images:
            pixel_values, num_patches_list = self._images_to_pixel_values(
                all_images, input_size=448, max_num=12
            )
            if pixel_values is not None:
                # Convert dtype/device as in docs
                pixel_values = pixel_values.to(torch.bfloat16)
                # Chat API expects tensors on CUDA when model is on CUDA
                try:
                    pixel_values = pixel_values.to(self.model.device)
                except Exception:
                    pass

        # Build question with any prior context and numbered image placeholders
        if all_images:
            # Separate images layout: Image-1: <image> ... then question text
            prefix_lines = [f"Image-{i+1}: <image>" for i in range(len(all_images))]
            prefix = "\n".join(prefix_lines) + "\n"
        else:
            prefix = ""

        last_user_text = "\n".join(last_user_text_parts).strip()
        # Combine prior text-only turns as context to emulate multi-turn
        context_text = "\n".join(context_lines[:-1]) if len(context_lines) > 1 else ""
        base_question = last_user_text if last_user_text else "Describe the image(s) in detail."
        if context_text:
            question = (context_text + "\n" + prefix + base_question).strip()
        else:
            question = (prefix + base_question).strip()

        # Generation config
        generation_config = dict(max_new_tokens=max_new_tokens, do_sample=False)

        # Call InternVL chat
        try:
            if pixel_values is None:
                # Pure-text conversation (embed prior turns in question)
                response = self.model.chat(self.tokenizer, None, question, generation_config)
            else:
                # Multi-image: pass num_patches_list if >1 image
                if len(num_patches_list) > 1:
                    response = self.model.chat(
                        self.tokenizer,
                        pixel_values,
                        question,
                        generation_config,
                        num_patches_list=num_patches_list,
                    )
                else:
                    response = self.model.chat(
                        self.tokenizer, pixel_values, question, generation_config
                    )
        except Exception as e:
            # Fallback: return empty string to avoid crashing the adapter
            return ""

        return response or ""
