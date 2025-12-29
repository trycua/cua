from typing import Optional

try:
    from transformers import AutoConfig

    HF_AVAILABLE = True
except ImportError:
    HF_AVAILABLE = False

from .generic import GenericHFModel
from .internvl import InternVLModel
from .opencua import OpenCUAModel
from .qwen2_5_vl import Qwen2_5_VLModel


def load_model(model_name: str, device: str = "auto", trust_remote_code: bool = False):
    """Factory function to load and return the right model handler instance.

    - If the underlying transformers config class matches OpenCUA, return OpenCUAModel
    - Otherwise, return GenericHFModel
    """
    if not HF_AVAILABLE:
        raise ImportError(
            'HuggingFace transformers dependencies not found. Install with: pip install "cua-agent[uitars-hf]"'
        )
    cfg = AutoConfig.from_pretrained(model_name, trust_remote_code=trust_remote_code)
    cls = cfg.__class__.__name__
    print(f"cls: {cls}")
    if "OpenCUA" in cls:
        return OpenCUAModel(
            model_name=model_name, device=device, trust_remote_code=trust_remote_code
        )
    elif "Qwen2_5_VL" in cls:
        return Qwen2_5_VLModel(
            model_name=model_name, device=device, trust_remote_code=trust_remote_code
        )
    elif "InternVL" in cls:
        return InternVLModel(
            model_name=model_name, device=device, trust_remote_code=trust_remote_code
        )
    return GenericHFModel(model_name=model_name, device=device, trust_remote_code=trust_remote_code)
