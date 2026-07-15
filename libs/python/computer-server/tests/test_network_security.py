"""Security regression tests for computer-server network exposure."""

from __future__ import annotations

import pytest
from computer_server.network_security import (
    InsecureBindError,
    is_loopback_host,
    resolve_bind_host,
)


@pytest.fixture
def clean_network_env(monkeypatch):
    for name in ("CONTAINER_NAME", "CUA_ALLOW_UNAUTHENTICATED_REMOTE"):
        monkeypatch.delenv(name, raising=False)
    return monkeypatch


@pytest.mark.parametrize("host", ["localhost", "127.0.0.1", "127.0.0.42", "::1", "[::1]"])
def test_loopback_hosts_are_recognized(host):
    assert is_loopback_host(host) is True


@pytest.mark.parametrize("host", ["0.0.0.0", "::", "192.168.1.10", "example.com", ""])
def test_remote_hosts_are_not_treated_as_loopback(host):
    assert is_loopback_host(host) is False


def test_local_mode_refuses_remote_bind(clean_network_env):
    with pytest.raises(InsecureBindError, match="local authentication is disabled"):
        resolve_bind_host("0.0.0.0")


def test_authenticated_container_allows_remote_bind(clean_network_env):
    clean_network_env.setenv("CONTAINER_NAME", "sandbox-1")
    assert resolve_bind_host("0.0.0.0") == "0.0.0.0"


def test_explicit_override_allows_remote_bind(clean_network_env):
    clean_network_env.setenv("CUA_ALLOW_UNAUTHENTICATED_REMOTE", "true")
    assert resolve_bind_host("0.0.0.0") == "0.0.0.0"


def test_programmatic_server_uses_same_bind_guard(clean_network_env):
    from computer_server.server import Server

    with pytest.raises(InsecureBindError):
        Server(host="0.0.0.0")


async def run_origin_guard(monkeypatch, *, origin=None, scope_type="http", container_name=None):
    from computer_server.main import LocalOriginGuard

    if container_name is None:
        monkeypatch.delenv("CONTAINER_NAME", raising=False)
    else:
        monkeypatch.setenv("CONTAINER_NAME", container_name)

    called = False
    sent = []

    async def inner(scope, receive, send):
        nonlocal called
        called = True

    headers = [] if origin is None else [(b"origin", origin.encode("latin-1"))]
    scope = {"type": scope_type, "path": "/mcp", "headers": headers}

    async def receive():
        return {"type": "websocket.connect"}

    async def send(message):
        sent.append(message)

    await LocalOriginGuard(inner)(scope, receive, send)
    return called, sent


@pytest.mark.asyncio
@pytest.mark.parametrize("origin", [None, "http://localhost:3000", "http://127.0.0.1"])
async def test_native_and_loopback_clients_reach_local_server(monkeypatch, origin):
    called, sent = await run_origin_guard(monkeypatch, origin=origin)
    assert called is True
    assert sent == []


@pytest.mark.asyncio
@pytest.mark.parametrize("origin", ["https://attacker.example", "null", "not a URL"])
async def test_cross_site_http_is_rejected_in_local_mode(monkeypatch, origin):
    called, sent = await run_origin_guard(monkeypatch, origin=origin)
    assert called is False
    assert sent[0]["status"] == 403


@pytest.mark.asyncio
async def test_cross_site_websocket_is_rejected_in_local_mode(monkeypatch):
    called, sent = await run_origin_guard(
        monkeypatch, origin="https://attacker.example", scope_type="websocket"
    )
    assert called is False
    assert sent == [{"type": "websocket.close", "code": 1008}]


@pytest.mark.asyncio
async def test_authenticated_container_keeps_external_origin_support(monkeypatch):
    called, sent = await run_origin_guard(
        monkeypatch, origin="https://control.cua.ai", container_name="sandbox-1"
    )
    assert called is True
    assert sent == []
