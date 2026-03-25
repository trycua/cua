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
class Qwen35Config(GenericVlmConfig):
    """Qwen3.5 agent loop using litellm with function/tool calling."""
    pass
