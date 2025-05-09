"""CUA (Computer Use) Agent for AI-driven computer interaction."""

import sys
import logging

__version__ = "0.1.31"

# Initialize logging
logger = logging.getLogger("cua.agent")

# Initialize telemetry when the package is imported
try:
    # Import from core telemetry for basic functions
    from core.telemetry import (
        is_telemetry_enabled,
        flush,
        record_event,
    )

    # Import set_dimension from our own telemetry module
    from .core.telemetry import set_dimension

    # Check if telemetry is enabled
    if is_telemetry_enabled():
        logger.info("Telemetry is enabled")

        # Record package initialization
        record_event(
            "module_init",
            {
                "module": "agent",
                "version": __version__,
                "python_version": sys.version,
            },
        )

        # Set the package version as a dimension
        set_dimension("agent_version", __version__)

        # Flush events to ensure they're sent
        flush()
    else:
        logger.info("Telemetry is disabled")
except ImportError as e:
    # Telemetry not available
    logger.warning(f"Telemetry not available: {e}")
except Exception as e:
    # Other issues with telemetry
    logger.warning(f"Error initializing telemetry: {e}")

from .core.types import LLMProvider, LLM
from .core.factory import AgentLoop
from .core.agent import ComputerAgent

__all__ = ["AgentLoop", "LLMProvider", "LLM", "ComputerAgent"]
