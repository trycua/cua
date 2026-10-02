"""``sb.mcp_config`` / ``sb.mcp``: the endpoint comes from the native SDK and
the client is the official ``mcp`` package. The in-test server speaks the
2025-06-18 streamable HTTP revision (sessions, SSE replies), which the SDK
negotiates down to. Skipped without the native ``cua`` binding or ``mcp``."""

from __future__ import annotations

import base64
import http.server
import json
import threading
import uuid
from types import SimpleNamespace

import pytest
from cua_sandbox.interfaces import mcp as mcp_mod
from cua_sandbox.transport.env import EnvTransport

PNG = base64.b64encode(b"\x89PNG\r\n\x1a\n" + bytes(range(256)) * 400).decode()


class _Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    sessions: set = set()
    seen_headers: list = []

    def log_message(self, *args):
        pass

    def _reply(self, code, body=b"", ctype=None, extra=()):
        self.send_response(code)
        if ctype:
            self.send_header("content-type", ctype)
        for k, v in extra:
            self.send_header(k, v)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self._reply(405)

    def do_DELETE(self):
        self.sessions.discard(self.headers.get("mcp-session-id"))
        self._reply(200)

    def do_POST(self):
        self.seen_headers.append(dict(self.headers))
        m = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
        extra = []
        method = m.get("method")
        if method == "initialize":
            sid = uuid.uuid4().hex
            self.sessions.add(sid)
            extra.append(("mcp-session-id", sid))
            result = {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "py-fake", "version": "1"},
            }
        elif self.headers.get("mcp-session-id") not in self.sessions:
            body = json.dumps(
                {"jsonrpc": "2.0", "id": m.get("id"), "error": {"code": -32601, "message": "no"}}
            ).encode()
            return self._reply(400, body, "application/json")
        elif "id" not in m:
            return self._reply(202)
        elif method == "tools/list":
            result = {"tools": [{"name": "shot", "inputSchema": {"type": "object"}}]}
        elif method == "tools/call":
            result = {
                "content": [
                    {"type": "text", "text": "a screenshot"},
                    {"type": "image", "data": PNG, "mimeType": "image/png"},
                    {"type": "audio", "data": "UklGRg==", "mimeType": "audio/wav"},
                ],
                "structuredContent": {"w": 1},
            }
        else:
            result = {}
        body = json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": result})
        self._reply(200, f"event: message\ndata: {body}\n\n".encode(), "text/event-stream", extra)


class _NativeService:
    def __init__(self, url, headers):
        self.url, self.headers, self.paths = url, headers, []

    async def mcp_config(self, path):
        self.paths.append(path)
        return SimpleNamespace(
            url=self.url + path,
            headers=[SimpleNamespace(name=k, value=v) for k, v in self.headers.items()],
        )


@pytest.fixture()
def server():
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{srv.server_address[1]}"
    srv.shutdown()


@pytest.mark.asyncio
async def test_config_and_official_client_keep_content_lossless(server):
    pytest.importorskip("mcp")
    svc = _NativeService(server, {"x-cua-fleet-claim": "c-1"})

    async def resolve():
        return svc

    cfg = await mcp_mod.mcp_config(resolve, "/mcp")
    assert cfg == {"url": server + "/mcp", "headers": {"x-cua-fleet-claim": "c-1"}}
    async with mcp_mod.open_mcp(resolve) as client:
        tools = await client.list_tools()
        tools = getattr(tools, "tools", tools)
        assert [t.name for t in tools] == ["shot"]
        r = await client.call_tool("shot", {})
        raw = r.model_dump(by_alias=True, exclude_none=True)
        assert [c["type"] for c in raw["content"]] == ["text", "image", "audio"]
        assert raw["content"][1] == {"type": "image", "data": PNG, "mimeType": "image/png"}
        assert raw["content"][2]["mimeType"] == "audio/wav"
        assert raw["structuredContent"] == {"w": 1}
    assert all(h.get("x-cua-fleet-claim") == "c-1" for h in _Handler.seen_headers)


@pytest.mark.asyncio
async def test_url_sandbox_resolves_the_native_service(server):
    pytest.importorskip("mcp")
    from cua_sandbox._sdk import native

    try:
        native()
    except ImportError:
        pytest.skip("the native cua binding is not installed")
    transport = EnvTransport(url=server)
    svc = await transport.native_service("env")
    cfg = mcp_mod._config_dict(await svc.mcp_config("/mcp"))
    assert cfg["url"] == server + "/mcp"
    async with mcp_mod.connect(cfg) as client:
        r = await client.call_tool("shot", {})
        assert r.content[1].data == PNG
    await transport.disconnect()


@pytest.mark.asyncio
async def test_fleet_transport_checks_declared_services():
    from cua_sandbox.transport.fleet import FleetTransport

    class Handle:
        def service(self, name):
            return name

    transport = FleetTransport.__new__(FleetTransport)
    transport._bound = SimpleNamespace(services=["mcp"], name="sb")

    async def fleet_handle():
        return Handle()

    transport._fleet_handle = fleet_handle
    assert await transport.native_service("mcp") == "mcp"
    with pytest.raises(ValueError, match="does not expose service 'env'"):
        await transport.native_service("env")


@pytest.mark.asyncio
async def test_live_reference_everything_server():
    """Opt-in: ``CUA_E2E_MCP_EVERYTHING_URL`` is the reference "everything"
    server's MCP endpoint (for example a memory-capped container of
    libs/cua/crates/cua-sdk/tests/fixtures/mcp-everything)."""
    import os

    url = os.environ.get("CUA_E2E_MCP_EVERYTHING_URL")
    if not url:
        pytest.skip("set CUA_E2E_MCP_EVERYTHING_URL")
    pytest.importorskip("mcp")
    async with mcp_mod.connect({"url": url, "headers": {}}) as client:
        r = await client.call_tool("get-tiny-image", {})
        raw = r.model_dump(by_alias=True, exclude_none=True)
        img = next(c for c in raw["content"] if c["type"] == "image")
        assert img["mimeType"] == "image/png"
        assert base64.b64decode(img["data"])[:8] == b"\x89PNG\r\n\x1a\n"
        r = await client.call_tool("get-resource-links", {"count": 2})
        raw = r.model_dump(by_alias=True, exclude_none=True)
        assert sum(c["type"] == "resource_link" for c in raw["content"]) == 2
