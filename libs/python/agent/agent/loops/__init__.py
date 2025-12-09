"""
Agent loops for agent
"""

# Import the loops to register them
from . import (
    anthropic,
    composed_grounded,
    gelato,
    gemini,
    glm45v,
    gta1,
    holo,
    internvl,
    moondream3,
    omniparser,
    openai,
    opencua,
    generic_vlm,
    uiins,
    uitars,
    uitars2,
)

__all__ = [
    "anthropic",
    "openai",
    "uitars",
    "omniparser",
    "gta1",
    "composed_grounded",
    "glm45v",
    "opencua",
    "internvl",
    "holo",
    "moondream3",
    "gemini",
    "generic_vlm",
    "uiins",
    "gelato",
    "uitars2",
]
