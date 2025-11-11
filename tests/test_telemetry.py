"""
Required environment variables:
- CUA_API_KEY: API key for Cua cloud provider
"""

import os
import sys
from pathlib import Path

import pytest

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.insert(0, path)  # Insert at beginning to prioritize
        print(f"Added to sys.path: {path}")

from core.telemetry import destroy_telemetry_client, is_telemetry_enabled, record_event


class TestTelemetry:
    def setup_method(self):
        """Reset environment variables before each test"""
        os.environ.pop("CUA_TELEMETRY", None)
        os.environ.pop("CUA_TELEMETRY_ENABLED", None)
        destroy_telemetry_client()

    def test_telemetry_disabled_when_cua_telemetry_is_off(self):
        """Should return false when CUA_TELEMETRY is off"""
        os.environ["CUA_TELEMETRY"] = "off"
        assert is_telemetry_enabled() is False

    def test_telemetry_enabled_when_cua_telemetry_not_set(self):
        """Should return true when CUA_TELEMETRY is not set"""
        assert is_telemetry_enabled() is True

    def test_telemetry_disabled_when_cua_telemetry_enabled_is_0(self):
        """Should return false if CUA_TELEMETRY_ENABLED is 0"""
        os.environ["CUA_TELEMETRY_ENABLED"] = "0"
        assert is_telemetry_enabled() is False

    def test_send_test_event_to_posthog(self):
        """Should send a test event to PostHog"""
        # This should not raise an exception
        record_event("test_telemetry", {"message": "Hello, world!"})


if __name__ == "__main__":
    # Run tests directly
    pytest.main([__file__, "-v"])
