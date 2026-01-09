"""Snapshot processors for converting batch outputs into various dataset formats."""

from .aguvis_stage_1 import AgUVisStage1Processor
from .base import BaseProcessor
from .gui_r1 import GuiR1Processor

# Registry of available processors
PROCESSORS = {
    "aguvis-stage-1": AgUVisStage1Processor,
    "gui-r1": GuiR1Processor,
}


def get_processor(name: str) -> type[BaseProcessor]:
    """Get a processor class by name."""
    if name not in PROCESSORS:
        available = ", ".join(PROCESSORS.keys())
        raise ValueError(f"Unknown processor: {name}. Available: {available}")
    return PROCESSORS[name]
