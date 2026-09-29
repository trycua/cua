"""OrcaRouter agent loop configuration.

OrcaRouter is an OpenAI-compatible gateway, so the general function/tool-calling
loop handles it. Re-exporting :class:`GenericVlmConfig` under an explicit
``orcarouter/`` pattern documents the provider as a first-class entry in the
loop registry instead of leaving it to the generic catch-all, and lets future
OrcaRouter-specific behaviour land here.
"""

from __future__ import annotations

from ..decorators import register_agent
from .generic_vlm import GenericVlmConfig


@register_agent(models=r"^orcarouter/", priority=1)
class OrcaRouterConfig(GenericVlmConfig):
    """Agent loop for OrcaRouter models (``orcarouter/<vendor>/<model>``)."""

    pass
