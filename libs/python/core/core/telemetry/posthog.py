"""Telemetry client using PostHog for collecting anonymous usage data."""

from __future__ import annotations

import logging
import os
import sys
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional

import posthog
from core import __version__

logger = logging.getLogger("core.telemetry")

# Public PostHog config for anonymous telemetry
# These values are intentionally public and meant for anonymous telemetry only
# https://posthog.com/docs/product-analytics/troubleshooting#is-it-ok-for-my-api-key-to-be-exposed-and-public
PUBLIC_POSTHOG_API_KEY = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14"
PUBLIC_POSTHOG_HOST = "https://eu.i.posthog.com"


class PostHogTelemetryClient:
    """Collects and reports telemetry data via PostHog."""

    # Global singleton (class-managed)
    _singleton: Optional["PostHogTelemetryClient"] = None

    def __init__(self):
        """Initialize PostHog telemetry client."""
        self.installation_id = self._get_or_create_installation_id()
        self.initialized = False
        self.queued_events: List[Dict[str, Any]] = []

        # Log telemetry status on startup
        if self.is_telemetry_enabled():
            logger.info("Telemetry enabled")
            # Initialize PostHog client if config is available
            self._initialize_posthog()
        else:
            logger.info("Telemetry disabled")

    @classmethod
    def is_telemetry_enabled(cls) -> bool:
        """True if telemetry is currently active for this process."""
        return os.environ.get("CUA_TELEMETRY_ENABLED", "true").lower() in {
            "1",
            "true",
            "yes",
            "on",
        }

    def _get_or_create_installation_id(self) -> str:
        """Get or create a unique installation ID that persists across runs.

        The ID is always stored within the core library directory itself,
        ensuring it persists regardless of how the library is used.

        This ID is not tied to any personal information.
        """
        # Get the core library directory (where this file is located)
        try:
            # Find the core module directory using this file's location
            core_module_dir = Path(
                __file__
            ).parent.parent  # core/telemetry/posthog_client.py -> core/telemetry -> core
            storage_dir = core_module_dir / ".storage"
            storage_dir.mkdir(exist_ok=True)

            id_file = storage_dir / "installation_id"

            # Try to read existing ID
            if id_file.exists():
                try:
                    stored_id = id_file.read_text().strip()
                    if stored_id:  # Make sure it's not empty
                        logger.debug(f"Using existing installation ID: {stored_id}")
                        return stored_id
                except Exception as e:
                    logger.debug(f"Error reading installation ID file: {e}")

            # Create new ID
            new_id = str(uuid.uuid4())
            try:
                id_file.write_text(new_id)
                logger.debug(f"Created new installation ID: {new_id}")
                return new_id
            except Exception as e:
                logger.warning(f"Could not write installation ID: {e}")
        except Exception as e:
            logger.warning(f"Error accessing core module directory: {e}")

        # Last resort: Create a new in-memory ID
        logger.warning("Using random installation ID (will not persist across runs)")
        return str(uuid.uuid4())

    def _initialize_posthog(self) -> bool:
        """Initialize the PostHog client with configuration.

        Returns:
            bool: True if initialized successfully, False otherwise
        """
        if self.initialized:
            return True

        try:
            # Allow overrides from environment for testing/region control
            posthog.api_key = PUBLIC_POSTHOG_API_KEY
            posthog.host = PUBLIC_POSTHOG_HOST

            # Configure the client
            posthog.debug = os.environ.get("CUA_TELEMETRY_DEBUG", "").lower() == "on"

            # Log telemetry status
            logger.info(
                f"Initializing PostHog telemetry with installation ID: {self.installation_id}"
            )
            if posthog.debug:
                logger.debug(f"PostHog API Key: {posthog.api_key}")
                logger.debug(f"PostHog Host: {posthog.host}")

            # Identify this installation
            self._identify()

            # Process any queued events
            for event in self.queued_events:
                posthog.capture(
                    distinct_id=self.installation_id,
                    event=event["event"],
                    properties=event["properties"],
                )
            self.queued_events = []

            self.initialized = True
            return True
        except Exception as e:
            logger.warning(f"Failed to initialize PostHog: {e}")
            return False

    def _identify(self) -> None:
        """Set up user properties for the current installation with PostHog."""
        try:
            properties = {
                "version": __version__,
                "is_ci": "CI" in os.environ,
                "os": os.name,
                "python_version": sys.version.split()[0],
            }

            logger.debug(
                f"Setting up PostHog user properties for: {self.installation_id} with properties: {properties}"
            )

            # In the Python SDK, we capture an identification event instead of calling identify()
            posthog.capture(
                distinct_id=self.installation_id, event="$identify", properties={"$set": properties}
            )

            logger.info(f"Set up PostHog user properties for installation: {self.installation_id}")
        except Exception as e:
            logger.warning(f"Failed to set up PostHog user properties: {e}")

    def record_event(self, event_name: str, properties: Optional[Dict[str, Any]] = None) -> None:
        """Record an event with optional properties.

        Args:
            event_name: Name of the event
            properties: Event properties (must not contain sensitive data)
        """
        # Respect runtime telemetry opt-out.
        if not self.is_telemetry_enabled():
            logger.debug("Telemetry disabled; event not recorded.")
            return

        event_properties = {"version": __version__, **(properties or {})}

        logger.info(f"Recording event: {event_name} with properties: {event_properties}")

        if self.initialized:
            try:
                posthog.capture(
                    distinct_id=self.installation_id, event=event_name, properties=event_properties
                )
                logger.info(f"Sent event to PostHog: {event_name}")
                # Flush immediately to ensure delivery
                posthog.flush()
            except Exception as e:
                logger.warning(f"Failed to send event to PostHog: {e}")
        else:
            # Queue the event for later
            logger.info(f"PostHog not initialized, queuing event for later: {event_name}")
            self.queued_events.append({"event": event_name, "properties": event_properties})
            # Try to initialize now if not already
            initialize_result = self._initialize_posthog()
            logger.info(f"Attempted to initialize PostHog: {initialize_result}")

    def flush(self) -> bool:
        """Flush any pending events to PostHog.

        Returns:
            bool: True if successful, False otherwise
        """
        if not self.initialized and not self._initialize_posthog():
            return False

        try:
            posthog.flush()
            return True
        except Exception as e:
            logger.debug(f"Failed to flush PostHog events: {e}")
            return False

    @classmethod
    def get_client(cls) -> "PostHogTelemetryClient":
        """Return the global PostHogTelemetryClient instance, creating it if needed."""
        if cls._singleton is None:
            cls._singleton = cls()
        return cls._singleton

    @classmethod
    def destroy_client(cls) -> None:
        """Destroy the global PostHogTelemetryClient instance."""
        cls._singleton = None


def destroy_telemetry_client() -> None:
    """Destroy the global PostHogTelemetryClient instance (class-managed)."""
    PostHogTelemetryClient.destroy_client()


def is_telemetry_enabled() -> bool:
    return PostHogTelemetryClient.is_telemetry_enabled()


def record_event(event_name: str, properties: Optional[Dict[str, Any]] | None = None) -> None:
    """Record an arbitrary PostHog event."""
    PostHogTelemetryClient.get_client().record_event(event_name, properties or {})
