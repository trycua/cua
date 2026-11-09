"""Telemetry client using PostHog for collecting anonymous usage data."""

from __future__ import annotations

import logging
import os
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional

import posthog
from cua_core import __version__
from cua_core.telemetry._config import telemetry_enabled_from_env

logger = logging.getLogger("core.telemetry")

# Public PostHog config for anonymous telemetry
# These values are intentionally public and meant for anonymous telemetry only
# https://posthog.com/docs/product-analytics/troubleshooting#is-it-ok-for-my-api-key-to-be-exposed-and-public
PUBLIC_POSTHOG_API_KEY = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14"
PUBLIC_POSTHOG_HOST = "https://eu.i.posthog.com"


class PostHogTelemetryClient:
    """Collects and reports anonymous telemetry data via PostHog.

    Privacy rules enforced here:

    * enablement follows :func:`cua_core.telemetry._config.telemetry_enabled_from_env`
      (``DO_NOT_TRACK``, ``CUA_TELEMETRY``, legacy switches, CI default off)
    * a dedicated ``posthog.Posthog`` client is used with GeoIP disabled; the
      global ``posthog`` module configuration is never touched
    * every event carries ``$process_person_profile: False``; no ``$identify``
    * the installation id file is only created while telemetry is enabled
    * the installation id, API key and event properties are only logged at DEBUG
    """

    # Global singleton (class-managed)
    _singleton: Optional["PostHogTelemetryClient"] = None

    def __init__(self):
        """Initialize PostHog telemetry client."""
        self.installation_id: Optional[str] = None
        self.initialized = False
        self.queued_events: List[Dict[str, Any]] = []
        self._client: Optional[Any] = None

        if self.is_telemetry_enabled():
            logger.debug("Telemetry enabled")
            self.installation_id = self._get_or_create_installation_id()
            self._initialize_posthog()
        else:
            logger.debug("Telemetry disabled")

    @classmethod
    def is_telemetry_enabled(cls) -> bool:
        """True if telemetry is currently active for this process."""
        return telemetry_enabled_from_env()

    def _get_or_create_installation_id(self) -> str:
        """Get or create a random installation id that persists across runs.

        Stored in ``~/.config/cua/installation_id`` so that the id survives
        package upgrades and is shared across venvs. It is a random UUID and is
        not derived from any personal information. Only called while telemetry
        is enabled.
        """
        try:
            config_dir = Path.home() / ".config" / "cua"
            id_file = config_dir / "installation_id"

            if id_file.exists():
                try:
                    stored_id = id_file.read_text().strip()
                    if stored_id:
                        return stored_id
                except Exception as e:
                    logger.debug(f"Error reading installation id file: {type(e).__name__}")

            new_id = str(uuid.uuid4())
            try:
                config_dir.mkdir(parents=True, exist_ok=True)
                id_file.write_text(new_id)
                return new_id
            except Exception as e:
                logger.debug(f"Could not write installation id: {type(e).__name__}")
        except Exception as e:
            logger.debug(f"Error accessing cua config directory: {type(e).__name__}")

        # Last resort: in-memory id (does not persist across runs)
        return str(uuid.uuid4())

    def _create_client(self) -> Any:
        """Create a dedicated PostHog client (never the global module client)."""
        return posthog.Posthog(
            PUBLIC_POSTHOG_API_KEY,
            host=PUBLIC_POSTHOG_HOST,
            disable_geoip=True,
            debug=os.environ.get("CUA_TELEMETRY_DEBUG", "").lower() == "on",
        )

    def _capture(self, event_name: str, properties: Dict[str, Any]) -> None:
        assert self._client is not None
        self._client.capture(
            distinct_id=self.installation_id,
            event=event_name,
            properties=properties,
            disable_geoip=True,
        )

    def _initialize_posthog(self) -> bool:
        """Initialize the PostHog client.

        Returns:
            bool: True if initialized successfully, False otherwise
        """
        if self.initialized:
            return True
        if not self.is_telemetry_enabled():
            return False

        try:
            if self.installation_id is None:
                self.installation_id = self._get_or_create_installation_id()
            self._client = self._create_client()

            for event in self.queued_events:
                self._capture(event["event"], event["properties"])
            self.queued_events = []

            self.initialized = True
            return True
        except Exception as e:
            logger.debug(f"Failed to initialize PostHog: {type(e).__name__}")
            return False

    def record_event(self, event_name: str, properties: Optional[Dict[str, Any]] = None) -> None:
        """Record an event with optional properties.

        Args:
            event_name: Name of the event
            properties: Event properties (must not contain personal data)
        """
        if not self.is_telemetry_enabled():
            return

        event_properties = {
            "version": __version__,
            **(properties or {}),
            "$process_person_profile": False,
            "$geoip_disable": True,
        }
        logger.debug(f"Recording telemetry event: {event_name}")

        if self.initialized:
            try:
                self._capture(event_name, event_properties)
                # Flush immediately to ensure delivery for short-lived processes
                self._client.flush()
            except Exception as e:
                logger.debug(f"Failed to send telemetry event: {type(e).__name__}")
        else:
            self.queued_events.append({"event": event_name, "properties": event_properties})
            self._initialize_posthog()

    def flush(self) -> bool:
        """Flush any pending events to PostHog.

        Returns:
            bool: True if successful, False otherwise
        """
        if not self.initialized and not self._initialize_posthog():
            return False

        try:
            self._client.flush()
            return True
        except Exception as e:
            logger.debug(f"Failed to flush PostHog events: {type(e).__name__}")
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
        inst = cls._singleton
        cls._singleton = None
        if inst is not None and inst._client is not None:
            try:
                inst._client.shutdown()
            except Exception:
                pass


def destroy_telemetry_client() -> None:
    """Destroy the global PostHogTelemetryClient instance (class-managed)."""
    PostHogTelemetryClient.destroy_client()


def is_telemetry_enabled() -> bool:
    return PostHogTelemetryClient.is_telemetry_enabled()


def record_event(event_name: str, properties: Optional[Dict[str, Any]] | None = None) -> None:
    """Record an arbitrary PostHog event."""
    PostHogTelemetryClient.get_client().record_event(event_name, properties or {})
