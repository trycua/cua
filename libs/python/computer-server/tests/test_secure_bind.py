"""Tests for secure-by-default networking (issue #1892).

Covers two defenses:

1. ``resolve_bind_host`` — the server defaults to a loopback bind in local
   (unauthenticated) mode and refuses to bind a public interface unless the
   operator explicitly opts in, while preserving ``0.0.0.0`` in authenticated
   sandbox/cloud mode.

2. ``CrossSiteOriginGuard`` — a pure-ASGI middleware that rejects cross-site
   *browser* requests to the shell/file/PTY endpoints (cross-site WebSocket
   hijacking / CSRF), while leaving native clients and ``/mcp`` untouched.
"""

from __future__ import annotations

import pytest

try:
    from computer_server.cli import InsecureBindError, resolve_bind_host
except Exception as import_error:  # pragma: no cover - environment-dependent
    pytest.skip(
        f"computer_server.cli unavailable in this environment: {import_error}",
        allow_module_level=True,
    )


@pytest.fixture
def clean_env(monkeypatch):
    """Remove env vars that influence bind-host resolution for a clean baseline."""
    for var in ("CONTAINER_NAME", "CUA_ALLOW_INSECURE"):
        monkeypatch.delenv(var, raising=False)
    return monkeypatch


class TestResolveBindHost:
    """Unit tests for the fail-closed bind-host resolution."""

    def test_local_mode_defaults_to_loopback(self, clean_env):
        # No CONTAINER_NAME → auth disabled → must not expose the network.
        assert resolve_bind_host(None) == "127.0.0.1"

    def test_cloud_mode_defaults_to_all_interfaces(self, clean_env):
        # CONTAINER_NAME set → auth enforced → 0.0.0.0 is the intended default.
        clean_env.setenv("CONTAINER_NAME", "vm-abc")
        assert resolve_bind_host(None) == "0.0.0.0"

    @pytest.mark.parametrize("host", ["127.0.0.1", "localhost", "::1"])
    def test_explicit_loopback_allowed_in_local_mode(self, clean_env, host):
        assert resolve_bind_host(host) == host

    @pytest.mark.parametrize("host", ["0.0.0.0", "192.168.1.10", "::"])
    def test_explicit_public_host_refused_in_local_mode(self, clean_env, host):
        with pytest.raises(InsecureBindError):
            resolve_bind_host(host)

    def test_explicit_public_host_allowed_with_override(self, clean_env):
        clean_env.setenv("CUA_ALLOW_INSECURE", "1")
        assert resolve_bind_host("0.0.0.0") == "0.0.0.0"

    def test_explicit_public_host_allowed_in_cloud_mode(self, clean_env):
        # Auth is enforced in cloud mode, so an explicit public bind is fine.
        clean_env.setenv("CONTAINER_NAME", "vm-abc")
        assert resolve_bind_host("0.0.0.0") == "0.0.0.0"


async def _run_guard(path, *, origin=None, scope_type="http"):
    """Drive the guard once; return ``(inner_called, sent_messages)``."""
    try:
        from computer_server.main import CrossSiteOriginGuard
    except Exception as exc:  # pragma: no cover - environment-dependent
        pytest.skip(f"computer_server.main unavailable in this environment: {exc}")

    state = {"inner_called": False}

    async def inner(scope, receive, send):
        state["inner_called"] = True

    headers = []
    if origin is not None:
        headers.append((b"origin", origin.encode("latin-1")))
    scope = {"type": scope_type, "path": path, "headers": headers}

    sent = []

    async def send(message):
        sent.append(message)

    async def receive():
        return {"type": "websocket.connect"}

    await CrossSiteOriginGuard(inner)(scope, receive, send)
    return state["inner_called"], sent


class TestCrossSiteOriginGuard:
    """Standalone tests for the pure-ASGI cross-site origin guard."""

    @pytest.mark.asyncio
    async def test_no_origin_passes_through(self):
        # Native SDK clients / curl send no Origin header.
        called, sent = await _run_guard("/ws")
        assert called is True
        assert sent == []

    @pytest.mark.asyncio
    @pytest.mark.parametrize("origin", ["http://localhost:3000", "http://127.0.0.1:8080"])
    async def test_loopback_origin_passes_through(self, origin):
        called, _ = await _run_guard("/cmd", origin=origin)
        assert called is True

    @pytest.mark.asyncio
    async def test_cross_site_http_is_rejected(self):
        called, sent = await _run_guard("/cmd", origin="https://evil.example")
        assert called is False
        assert sent[0]["type"] == "http.response.start"
        assert sent[0]["status"] == 403

    @pytest.mark.asyncio
    async def test_cross_site_websocket_is_rejected(self):
        called, sent = await _run_guard(
            "/ws", origin="https://evil.example", scope_type="websocket"
        )
        assert called is False
        assert {"type": "websocket.close", "code": 1008} in sent

    @pytest.mark.asyncio
    async def test_pty_subpath_is_protected(self):
        called, _ = await _run_guard(
            "/pty/123/ws", origin="https://evil.example", scope_type="websocket"
        )
        assert called is False

    @pytest.mark.asyncio
    async def test_null_origin_is_rejected(self):
        # Sandboxed iframes and file:// pages send ``Origin: null``.
        called, _ = await _run_guard("/cmd", origin="null")
        assert called is False

    @pytest.mark.asyncio
    @pytest.mark.parametrize("path", ["/status", "/mcp", "/mcp/", "/responses"])
    async def test_unprotected_paths_ignore_origin(self, path):
        # /mcp and /status must keep working for cross-origin connectors.
        called, _ = await _run_guard(path, origin="https://evil.example")
        assert called is True
