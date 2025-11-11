"""
Agent loops for agent
"""

# Import the loops to register them
from . import (
    anthropic,
    composed_grounded,
    gemini,
    glm45v,
    gta1,
    holo,
    internvl,
    moondream3,
    omniparser,
    openai,
    opencua,
    qwen,
    uitars,
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
    "qwen",
]
