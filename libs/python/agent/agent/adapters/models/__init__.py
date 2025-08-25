from typing import Optional

try:
    from transformers import AutoConfig
    HF_AVAILABLE = True
except ImportError:
    HF_AVAILABLE = False

from .generic import GenericHFModel
from .opencua import OpenCUAModel


def load_model(model_name: str, device: str = "auto"):
    """Factory function to load and return the right model handler instance.
    
    - If the underlying transformers config class matches OpenCUA, return OpenCUAModel
    - Otherwise, return GenericHFModel
    """
    if not HF_AVAILABLE:
        raise ImportError(
            "HuggingFace transformers dependencies not found. Install with: pip install \"cua-agent[uitars-hf]\""
        )
    cfg = AutoConfig.from_pretrained(model_name, trust_remote_code=True)
    cls = cfg.__class__.__name__
    # print(f"cls: {cls}")
    if "OpenCUA" in cls:
        return OpenCUAModel(model_name=model_name, device=device)
    return GenericHFModel(model_name=model_name, device=device)
