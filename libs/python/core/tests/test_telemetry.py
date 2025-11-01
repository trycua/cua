"""Unit tests for core telemetry functionality.

This file tests ONLY telemetry logic, following SRP.
All external dependencies (PostHog, file system) are mocked.
"""

import os
from pathlib import Path
from unittest.mock import MagicMock, Mock, mock_open, patch

import pytest


class TestTelemetryEnabled:
    """Test telemetry enable/disable logic (SRP: Only tests enable/disable)."""

    def test_telemetry_enabled_by_default(self, monkeypatch):
        """Test that telemetry is enabled by default."""
        # Remove any environment variables that might affect the test
        monkeypatch.delenv("CUA_TELEMETRY", raising=False)
        monkeypatch.delenv("CUA_TELEMETRY_ENABLED", raising=False)

        from core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is True

    def test_telemetry_disabled_with_legacy_flag(self, monkeypatch):
        """Test that telemetry can be disabled with legacy CUA_TELEMETRY=off."""
        monkeypatch.setenv("CUA_TELEMETRY", "off")

        from core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    def test_telemetry_disabled_with_new_flag(self, monkeypatch):
        """Test that telemetry can be disabled with CUA_TELEMETRY_ENABLED=false."""
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "false")

        from core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("value", ["0", "false", "no", "off"])
    def test_telemetry_disabled_with_various_values(self, monkeypatch, value):
        """Test that telemetry respects various disable values."""
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", value)

        from core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is False

    @pytest.mark.parametrize("value", ["1", "true", "yes", "on"])
    def test_telemetry_enabled_with_various_values(self, monkeypatch, value):
        """Test that telemetry respects various enable values."""
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", value)

        from core.telemetry import is_telemetry_enabled

        assert is_telemetry_enabled() is True


class TestPostHogTelemetryClient:
    """Test PostHogTelemetryClient class (SRP: Only tests client logic)."""

    @patch("core.telemetry.posthog.posthog")
    @patch("core.telemetry.posthog.Path")
    def test_client_initialization(self, mock_path, mock_posthog, disable_telemetry):
        """Test that client initializes correctly."""
        from core.telemetry.posthog import PostHogTelemetryClient

        # Mock the storage directory
        mock_storage_dir = MagicMock()
        mock_storage_dir.exists.return_value = False
        mock_path.return_value.parent.parent = MagicMock()
        mock_path.return_value.parent.parent.__truediv__.return_value = mock_storage_dir

        # Reset singleton
        PostHogTelemetryClient.destroy_client()

        client = PostHogTelemetryClient()

        assert client is not None
        assert hasattr(client, "installation_id")
        assert hasattr(client, "initialized")
        assert hasattr(client, "queued_events")

    @patch("core.telemetry.posthog.posthog")
    @patch("core.telemetry.posthog.Path")
    def test_installation_id_generation(self, mock_path, mock_posthog, disable_telemetry):
        """Test that installation ID is generated if not exists."""
        from core.telemetry.posthog import PostHogTelemetryClient

        # Mock file system
        mock_id_file = MagicMock()
        mock_id_file.exists.return_value = False
        mock_storage_dir = MagicMock()
        mock_storage_dir.__truediv__.return_value = mock_id_file

        mock_core_dir = MagicMock()
        mock_core_dir.__truediv__.return_value = mock_storage_dir
        mock_path.return_value.parent.parent = mock_core_dir

        # Reset singleton
        PostHogTelemetryClient.destroy_client()

        client = PostHogTelemetryClient()

        # Should have generated a new UUID
        assert client.installation_id is not None
        assert len(client.installation_id) == 36  # UUID format

    @patch("core.telemetry.posthog.posthog")
    @patch("core.telemetry.posthog.Path")
    def test_installation_id_persistence(self, mock_path, mock_posthog, disable_telemetry):
        """Test that installation ID is read from file if exists."""
        from core.telemetry.posthog import PostHogTelemetryClient

        existing_id = "test-installation-id-123"

        # Mock file system
        mock_id_file = MagicMock()
        mock_id_file.exists.return_value = True
        mock_id_file.read_text.return_value = existing_id

        mock_storage_dir = MagicMock()
        mock_storage_dir.__truediv__.return_value = mock_id_file

        mock_core_dir = MagicMock()
        mock_core_dir.__truediv__.return_value = mock_storage_dir
        mock_path.return_value.parent.parent = mock_core_dir

        # Reset singleton
        PostHogTelemetryClient.destroy_client()

        client = PostHogTelemetryClient()

        assert client.installation_id == existing_id

    @patch("core.telemetry.posthog.posthog")
    @patch("core.telemetry.posthog.Path")
    def test_record_event_when_disabled(self, mock_path, mock_posthog, monkeypatch):
        """Test that events are not recorded when telemetry is disabled."""
        from core.telemetry.posthog import PostHogTelemetryClient

        # Disable telemetry explicitly using the correct environment variable
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "false")

        # Mock file system
        mock_storage_dir = MagicMock()
        mock_storage_dir.exists.return_value = False
        mock_path.return_value.parent.parent = MagicMock()
        mock_path.return_value.parent.parent.__truediv__.return_value = mock_storage_dir

        # Reset singleton
        PostHogTelemetryClient.destroy_client()

        client = PostHogTelemetryClient()
        client.record_event("test_event", {"key": "value"})

        # PostHog capture should not be called at all when telemetry is disabled
        mock_posthog.capture.assert_not_called()

    @patch("core.telemetry.posthog.posthog")
    @patch("core.telemetry.posthog.Path")
    def test_record_event_when_enabled(self, mock_path, mock_posthog, monkeypatch):
        """Test that events are recorded when telemetry is enabled."""
        from core.telemetry.posthog import PostHogTelemetryClient

        # Enable telemetry
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "true")

        # Mock file system
        mock_storage_dir = MagicMock()
        mock_storage_dir.exists.return_value = False
        mock_path.return_value.parent.parent = MagicMock()
        mock_path.return_value.parent.parent.__truediv__.return_value = mock_storage_dir

        # Reset singleton
        PostHogTelemetryClient.destroy_client()

        client = PostHogTelemetryClient()
        client.initialized = True  # Pretend it's initialized

        event_name = "test_event"
        event_props = {"key": "value"}
        client.record_event(event_name, event_props)

        # PostHog capture should be called
        assert mock_posthog.capture.call_count >= 1

    @patch("core.telemetry.posthog.posthog")
    @patch("core.telemetry.posthog.Path")
    def test_singleton_pattern(self, mock_path, mock_posthog, disable_telemetry):
        """Test that get_client returns the same instance."""
        from core.telemetry.posthog import PostHogTelemetryClient

        # Mock file system
        mock_storage_dir = MagicMock()
        mock_storage_dir.exists.return_value = False
        mock_path.return_value.parent.parent = MagicMock()
        mock_path.return_value.parent.parent.__truediv__.return_value = mock_storage_dir

        # Reset singleton
        PostHogTelemetryClient.destroy_client()

        client1 = PostHogTelemetryClient.get_client()
        client2 = PostHogTelemetryClient.get_client()

        assert client1 is client2


class TestRecordEvent:
    """Test the public record_event function (SRP: Only tests public API)."""

    @patch("core.telemetry.posthog.PostHogTelemetryClient")
    def test_record_event_calls_client(self, mock_client_class, disable_telemetry):
        """Test that record_event delegates to the client."""
        from core.telemetry import record_event

        mock_client_instance = Mock()
        mock_client_class.get_client.return_value = mock_client_instance

        event_name = "test_event"
        event_props = {"key": "value"}

        record_event(event_name, event_props)

        mock_client_instance.record_event.assert_called_once_with(event_name, event_props)

    @patch("core.telemetry.posthog.PostHogTelemetryClient")
    def test_record_event_without_properties(self, mock_client_class, disable_telemetry):
        """Test that record_event works without properties."""
        from core.telemetry import record_event

        mock_client_instance = Mock()
        mock_client_class.get_client.return_value = mock_client_instance

        event_name = "test_event"

        record_event(event_name)

        mock_client_instance.record_event.assert_called_once_with(event_name, {})


class TestDestroyTelemetryClient:
    """Test client destruction (SRP: Only tests cleanup)."""

    @patch("core.telemetry.posthog.PostHogTelemetryClient")
    def test_destroy_client_calls_class_method(self, mock_client_class):
        """Test that destroy_telemetry_client delegates correctly."""
        from core.telemetry import destroy_telemetry_client

        destroy_telemetry_client()

        mock_client_class.destroy_client.assert_called_once()
