"""PathBridge and Endpoints.host_port against local fake upstreams (hermetic)."""

from __future__ import annotations

import asyncio
import json
import shutil
import ssl
import subprocess
from types import SimpleNamespace

import pytest
from cua_bench.adapters import Endpoints, ServerSpec
from cua_bench.adapters.bridge import PathBridge


async def _upstream(ssl_ctx=None):
    """Echo the request line/Host as JSON, return a DevTools-like body, or
    accept a WebSocket upgrade and echo raw bytes."""
    seen = []

    async def handle(reader, writer):
        head = (await reader.readuntil(b"\r\n\r\n")).decode()
        first, *lines = head.split("\r\n")
        headers = {k.lower(): v.strip() for k, _, v in (ln.partition(":") for ln in lines if ln)}
        seen.append((first, headers))
        method, target, _ = first.split(" ")
        if headers.get("upgrade", "").lower() == "websocket":
            writer.write(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
            await writer.drain()
            for _ in range(10):  # bounded echo
                data = await reader.read(1024)
                if not data:
                    break
                writer.write(data[::-1])
                await writer.drain()
            writer.close()
            return
        length = int(headers.get("content-length", "0"))
        body_in = await reader.readexactly(length) if length else b""
        host = headers.get("host")
        if target.endswith("/json/version") or "/json/version?" in target:
            body = json.dumps({"webSocketDebuggerUrl": f"ws://{host}/devtools/browser/x"}).encode()
        else:
            body = json.dumps({"line": first, "host": host, "body": body_in.decode()}).encode()
        writer.write(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
            + f"Content-Length: {len(body)}\r\n\r\n".encode()
            + body
        )
        await writer.drain()
        # keep the connection open like a keep-alive server would
        await asyncio.sleep(0.05)
        writer.close()

    server = await asyncio.start_server(handle, "127.0.0.1", 0, ssl=ssl_ctx)
    return server, server.sockets[0].getsockname()[1], seen


async def _http(port, raw: bytes) -> bytes:
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(raw)
    await writer.drain()
    data = await asyncio.wait_for(reader.read(1 << 20), 5)
    rest = b""
    for _ in range(20):
        chunk = await asyncio.wait_for(reader.read(1 << 20), 5)
        if not chunk:
            break
        rest += chunk
    writer.close()
    return data + rest


def test_plain_http_prefix_query_host_and_body():
    async def run():
        server, port, seen = await _upstream()
        bridge = PathBridge(f"http://127.0.0.1:{port}/api/svc/ns/sb-port-5000?sig=abc")
        host, bport = await bridge.start()
        resp = await _http(
            bport,
            b"POST /setup/execute?x=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n"
            b"Content-Length: 5\r\n\r\nhello",
        )
        await bridge.aclose()
        server.close()
        return host, resp, seen

    host, resp, seen = asyncio.run(run())
    assert host == "127.0.0.1"
    head, _, body = resp.partition(b"\r\n\r\n")
    assert b"Connection: close" in head
    data = json.loads(body)
    assert data["line"] == "POST /api/svc/ns/sb-port-5000/setup/execute?x=1&sig=abc HTTP/1.1"
    assert data["body"] == "hello"
    assert seen[0][1]["connection"] == "close"


def test_devtools_json_points_back_through_the_bridge():
    async def run():
        server, port, _ = await _upstream()
        bridge = PathBridge(f"http://127.0.0.1:{port}/api/svc/ns/sb-port-9222")
        _, bport = await bridge.start()
        resp = await _http(bport, b"GET /json/version HTTP/1.1\r\nHost: x\r\n\r\n")
        await bridge.aclose()
        server.close()
        return bport, resp

    bport, resp = asyncio.run(run())
    body = json.loads(resp.partition(b"\r\n\r\n")[2])
    assert body["webSocketDebuggerUrl"] == f"ws://127.0.0.1:{bport}/devtools/browser/x"


def test_websocket_upgrade_is_piped():
    async def run():
        server, port, seen = await _upstream()
        bridge = PathBridge(f"http://127.0.0.1:{port}/p")
        _, bport = await bridge.start()
        reader, writer = await asyncio.open_connection("127.0.0.1", bport)
        writer.write(b"GET /devtools/page/1 HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
        await writer.drain()
        head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
        writer.write(b"abc")
        await writer.drain()
        echoed = await asyncio.wait_for(reader.readexactly(3), 5)
        writer.close()
        await bridge.aclose()
        server.close()
        return head, echoed, seen

    head, echoed, seen = asyncio.run(run())
    assert head.startswith(b"HTTP/1.1 101")
    assert echoed == b"cba"
    assert seen[0][0] == "GET /p/devtools/page/1 HTTP/1.1"


@pytest.mark.skipif(shutil.which("openssl") is None, reason="needs openssl to mint a test cert")
def test_https_upstream(tmp_path):
    cert, key = tmp_path / "c.pem", tmp_path / "k.pem"
    subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
         "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
         "-keyout", str(key), "-out", str(cert)],
        check=True, capture_output=True,
    )
    server_ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    server_ctx.load_cert_chain(cert, key)
    client_ctx = ssl.create_default_context(cafile=str(cert))

    async def run():
        server, port, _ = await _upstream(server_ctx)
        bridge = PathBridge(f"https://localhost:{port}/svc", ssl_context=client_ctx)
        _, bport = await bridge.start()
        resp = await _http(bport, b"GET /screenshot HTTP/1.1\r\nHost: x\r\n\r\n")
        await bridge.aclose()
        server.close()
        return port, resp

    port, resp = asyncio.run(run())
    data = json.loads(resp.partition(b"\r\n\r\n")[2])
    assert data["line"] == "GET /svc/screenshot HTTP/1.1"
    assert data["host"] == f"localhost:{port}"


class _Service:
    def __init__(self, url):
        self._url = url

    async def url(self):
        return self._url


def test_endpoints_host_port_local_and_bridged():
    urls = {"server": "http://127.0.0.1:41000", "port-9222": "https://gw.example/api/svc/n/s-port-9222?sig=1"}
    sandbox = SimpleNamespace(exposed_ports={}, service=lambda name: _Service(urls[name]))
    ep = Endpoints(SimpleNamespace(sandbox=sandbox), ServerSpec(port=5000))

    async def run():
        local = await ep.host_port("server")
        bridged = await ep.host_port(9222)
        again = await ep.host_port(9222)
        await ep.aclose()
        return local, bridged, again

    local, bridged, again = asyncio.run(run())
    assert local == ("127.0.0.1", 41000)
    assert bridged[0] == "127.0.0.1" and bridged == again  # one bridge per target
    assert ep._bridges == {}
