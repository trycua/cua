"""The Python binding's telemetry surface. Offline: telemetry is forced off
and the network guard is on; nothing here can send."""

import os
import socket

import pytest

os.environ["CUA_TELEMETRY"] = "0"
os.environ["CUA_TELEMETRY_FORBID_NETWORK"] = "1"
# DO_NOT_TRACK outranks CUA_TELEMETRY; CI sets it, so drop it to test the
# CUA_TELEMETRY source this file asserts on (telemetry stays off either way).
os.environ.pop("DO_NOT_TRACK", None)


@pytest.fixture(autouse=True)
def _home(tmp_path, monkeypatch):
    monkeypatch.setenv("CUA_HOME", str(tmp_path / ".cua"))
    real = socket.socket.connect

    def guard(self, addr):  # no network from these tests
        if isinstance(addr, tuple) and addr[0] not in ("127.0.0.1", "::1", "localhost"):
            raise AssertionError(f"network access in a test: {addr}")
        return real(self, addr)

    monkeypatch.setattr(socket.socket, "connect", guard)


def test_status_is_off_in_tests_and_names_the_python_surface():
    import cua.telemetry as t

    s = t.status()
    assert s.enabled is False
    assert s.source_kind == "env"
    assert s.product == "sdk_python"
    assert '"$geoip_disable":true' in s.envelope_json.replace(" ", "")


def test_schema_lists_the_teleport_caller_kind():
    import cua.telemetry as t

    events = {e["name"]: e for e in t.schema()["events"]}
    props = {p["name"]: p for p in events["cua_teleport_completed"]["properties"]}
    assert props["caller_kind"]["kind"]["enum"] == [
        "cua_app_signed",
        "embedded_sdk",
        "unsigned",
        "unknown",
    ]
    assert "requires_cua_app" in props["outcome"]["kind"]["enum"]


def test_data_sharing_is_not_exposed():
    import cua.telemetry as t
    from cua import _native

    # Opt-in data sharing is parked: no consent API in any binding.
    assert not hasattr(t, "data_sharing_status")
    assert not hasattr(_native, "data_sharing_grant")
    assert not hasattr(t.status(), "data_sharing_granted")
