"""
Adapters package for agent - Custom LLM adapters for LiteLLM
"""

from .azure_ml_adapter import AzureMLAdapter
from .cua_adapter import CUAAdapter
from .huggingfacelocal_adapter import HuggingFaceLocalAdapter
from .human_adapter import HumanAdapter
from .mlxvlm_adapter import MLXVLMAdapter
from .yutori_adapter import YutoriAdapter

__all__ = [
    "AzureMLAdapter",
    "HuggingFaceLocalAdapter",
    "HumanAdapter",
    "MLXVLMAdapter",
    "CUAAdapter",
    "YutoriAdapter",
]
