from typing import Any, Dict, List, Optional

# Hugging Face imports are local to avoid hard dependency at module import
try:
    import torch  # type: ignore
    from transformers import AutoModel, AutoProcessor  # type: ignore

    HF_AVAILABLE = True
except Exception:
    HF_AVAILABLE = False


class GenericHFModel:
    """Generic Hugging Face vision-language model handler.
    Loads an AutoModelForImageTextToText and AutoProcessor and generates text.
    """

    def __init__(
        self, model_name: str, device: str = "auto", trust_remote_code: bool = False
    ) -> None:
        if not HF_AVAILABLE:
            raise ImportError(
                'HuggingFace transformers dependencies not found. Install with: pip install "cua-agent[uitars-hf]"'
            )
        self.model_name = model_name
        self.device = device
        self.model = None
        self.processor = None
        self.trust_remote_code = trust_remote_code
        self._load()

    def _load(self) -> None:
        # Load model
        self.model = AutoModel.from_pretrained(
            self.model_name,
            torch_dtype=torch.float16,
            device_map=self.device,
            attn_implementation="sdpa",
            trust_remote_code=self.trust_remote_code,
        )
        # Load processor
        self.processor = AutoProcessor.from_pretrained(
            self.model_name,
            min_pixels=3136,
            max_pixels=4096 * 2160,
            device_map=self.device,
            trust_remote_code=self.trust_remote_code,
        )

    def generate(self, messages: List[Dict[str, Any]], max_new_tokens: int = 128) -> str:
        """Generate text for the given HF-format messages.
        messages: [{ role, content: [{type:'text'|'image', text|image}] }]
        """
        assert self.model is not None and self.processor is not None
        # Apply chat template and tokenize
        inputs = self.processor.apply_chat_template(
            messages,
            add_generation_prompt=True,
            tokenize=True,
            return_dict=True,
            return_tensors="pt",
        )
        # Move inputs to the same device as model
        inputs = inputs.to(self.model.device)
        # Generate
        with torch.no_grad():
            generated_ids = self.model.generate(**inputs, max_new_tokens=max_new_tokens)
        # Trim prompt tokens from output
        generated_ids_trimmed = [
            out_ids[len(in_ids) :] for in_ids, out_ids in zip(inputs.input_ids, generated_ids)
        ]
        # Decode
        output_text = self.processor.batch_decode(
            generated_ids_trimmed,
            skip_special_tokens=True,
            clean_up_tokenization_spaces=False,
        )
        return output_text[0] if output_text else ""
