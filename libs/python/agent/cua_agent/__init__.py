"""
agent - Decorator-based Computer Use Agent with liteLLM integration
"""

import logging
import sys

# Import loops to register them
from . import loops
from .agent import ComputerAgent
from .decorators import register_agent
from .types import AgentResponse, Messages

__all__ = ["register_agent", "ComputerAgent", "Messages", "AgentResponse"]

__version__ = "0.4.0"

logger = logging.getLogger(__name__)

# Initialize telemetry when the package is imported
try:
    # Import from core telemetry for basic functions
    from cua_core.telemetry import (
        is_telemetry_enabled,
        record_event,
    )

    # Check if telemetry is enabled
    if is_telemetry_enabled():
        logger.debug("Telemetry is enabled")

        # Record package initialization
        record_event(
            "module_init",
            {
                "module": "agent",
                "version": __version__,
                "python_version": f"{sys.version_info.major}.{sys.version_info.minor}",
            },
        )

    else:
        logger.debug("Telemetry is disabled")
except ImportError as e:
    # Telemetry not available
    logger.debug(f"Telemetry not available: {type(e).__name__}")
except Exception as e:
    # Other issues with telemetry
    logger.debug(f"Error initializing telemetry: {type(e).__name__}")
