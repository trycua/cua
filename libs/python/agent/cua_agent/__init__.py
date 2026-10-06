"""
agent - Decorator-based Computer Use Agent with liteLLM integration
"""

import logging
import sys
import warnings

# cua-agent is no longer maintained. Python's default filters show this once,
# when the importing code is __main__; `-W error::DeprecationWarning` turns it
# on everywhere. Importing still works exactly as before.
warnings.warn(
    "cua-agent is deprecated: Cua no longer maintains it and it will not receive updates or fixes. "
    "For desktop control, connect your coding agent to Cua Driver "
    "(https://cua.ai/docs/cua-driver/guides/connect-your-agent) over MCP; "
    "for sandboxes, use the cua SDK (pip install cua). "
    "Migration guide: https://cua.ai/docs/cua-sdk/guides/migrate-from-deprecated-packages",
    DeprecationWarning,
    stacklevel=2,
)

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
