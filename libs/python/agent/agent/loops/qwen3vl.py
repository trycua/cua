"""
Qwen3-VL dedicated agent loop configuration.
Re-exports GenericVlmConfig under a Qwen-specific model pattern so that
Qwen model strings are matched at normal priority instead of the generic
catch-all (priority -100).
"""

from __future__ import annotations

from ..decorators import register_agent
from .generic_vlm import GenericVlmConfig


@register_agent(models=r"(?i).*qwen.*")
class Qwen3VlConfig(GenericVlmConfig):
    """Qwen3-VL agent loop using litellm with function/tool calling."""
    pass
