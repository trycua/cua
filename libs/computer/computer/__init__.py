"""CUA Computer Interface for cross-platform computer control."""

import logging
import sys

__version__ = "0.1.0"

# Initialize logging
logger = logging.getLogger("cua.computer")

# Initialize telemetry when the package is imported
try:
    # Import from core telemetry
    from core.telemetry import (
        is_telemetry_enabled,
        flush,
        record_event,
    )

    # Check if telemetry is enabled
    if is_telemetry_enabled():
        logger.info("Telemetry is enabled")

        # Record package initialization
        record_event(
            "module_init",
            {
                "module": "computer",
                "version": __version__,
                "python_version": sys.version,
            },
        )

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

# Core components
from .computer import Computer

# Provider components
from .providers.base import VMProviderType

__all__ = ["Computer", "VMProviderType"]
