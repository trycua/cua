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
    from core.telemetry import (
        is_telemetry_enabled,
        record_event,
    )

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

    else:
        logger.info("Telemetry is disabled")
except ImportError as e:
    # Telemetry not available
    logger.warning(f"Telemetry not available: {e}")
except Exception as e:
    # Other issues with telemetry
    logger.warning(f"Error initializing telemetry: {e}")
