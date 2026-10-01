"""Offline checks of the shared Cua telemetry switches (cua-core).

Nothing here sends an event: the PostHog client is replaced with a mock,
HOME points at a temp dir, and any socket connect fails the test.
"""

import socket
from unittest.mock import MagicMock

import pytest
from cua_core.telemetry import (
    destroy_telemetry_client,
    is_telemetry_enabled,
    record_event,
)
from cua_core.telemetry._config import CI_ENV_VARS

_SWITCHES = ("DO_NOT_TRACK", "CUA_TELEMETRY", "CUA_TELEMETRY_ENABLED", "CUA_TELEMETRY_DISABLED")


@pytest.fixture(autouse=True)
def offline(monkeypatch, tmp_path):
    for name in (*_SWITCHES, *CI_ENV_VARS):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))

    def _blocked(*_a, **_k):
        raise AssertionError("network access attempted during telemetry test")

    monkeypatch.setattr(socket.socket, "connect", _blocked)
    monkeypatch.setattr(socket, "create_connection", _blocked)

    from cua_core.telemetry import posthog as ph_mod

    client = MagicMock(name="PosthogClient")
    monkeypatch.setattr(ph_mod.posthog, "Posthog", MagicMock(return_value=client))
    destroy_telemetry_client()
    yield client
    destroy_telemetry_client()


class TestTelemetry:
    def test_disabled_when_cua_telemetry_is_off(self, monkeypatch):
        monkeypatch.setenv("CUA_TELEMETRY", "off")
        assert is_telemetry_enabled() is False

    def test_enabled_when_nothing_set(self):
        assert is_telemetry_enabled() is True

    def test_disabled_when_cua_telemetry_enabled_is_0(self, monkeypatch):
        monkeypatch.setenv("CUA_TELEMETRY_ENABLED", "0")
        assert is_telemetry_enabled() is False

    def test_disabled_by_do_not_track(self, monkeypatch):
        monkeypatch.setenv("DO_NOT_TRACK", "1")
        assert is_telemetry_enabled() is False

    def test_off_in_ci_unless_opted_in(self, monkeypatch):
        monkeypatch.setenv("CI", "true")
        assert is_telemetry_enabled() is False
        monkeypatch.setenv("CUA_TELEMETRY", "1")
        assert is_telemetry_enabled() is True

    def test_record_event_uses_mocked_client_only(self, offline):
        record_event("test_telemetry", {"k": 1})
        assert offline.capture.call_count == 1
        props = offline.capture.call_args.kwargs["properties"]
        assert props["$process_person_profile"] is False

    def test_record_event_noop_when_disabled(self, offline, monkeypatch, tmp_path):
        monkeypatch.setenv("CUA_TELEMETRY", "off")
        record_event("test_telemetry", {"k": 1})
        offline.capture.assert_not_called()
        assert not (tmp_path / ".config" / "cua" / "installation_id").exists()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
