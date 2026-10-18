"""LIVE: the unified Sandbox API on a plain image, local (gVisor) and cloud.

The audit's probe scenario: ``python:3.12-slim`` running an MCP-style
server as ``command=``, declared as ``services={"mcp": 8765}`` with an
``http("mcp", "/health")`` readiness probe, then

* ``sb.service("mcp").request`` with MCP headers (``accept``,
  ``mcp-session-id``; the server answers 400 when they are dropped),
* ``sb.service("mcp").url()`` and ``sb.public_url("mcp", ttl)`` used by a
  plain HTTP client with no credentials,
* ``sb.tunnel.forward(8765)``.

Also: image layers (``pip_install``, ``run``, ``env``, ``copy``) on the same
plain image with ``local=True`` build into the container engine before boot
and are reused by an identical second sandbox.

Opt-in (both create real resources, named ``cua-e2e-*`` and removed in
``finally``)::

    CUA_TEST_UNIFIED_LOCAL=1 .venv/bin/python -m pytest -q tests/live/test_unified_api_live.py
    set -a; source ~/.env; set +a
    CUA_TEST_UNIFIED_FLEET=1 .venv/bin/python -m pytest -q tests/live/test_unified_api_live.py

The local run starts a cua daemon for the public URL under a throwaway
``CUA_HOME`` (``CUA_BIN`` must name the CLI) and stops it afterwards.
"""

from __future__ import annotations

import os
import secrets
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

import httpx
import pytest
from cua_sandbox import Image, Sandbox, _autopool, http

REPO = Path(__file__).resolve().parents[5]
SERVER = (REPO / "tests" / "e2e" / "cua-sdk" / "fixtures" / "mcp_probe_server.py").read_text()
HEADERS = {"accept": "application/json, text/event-stream", "content-type": "application/json"}


def _on(var: str) -> bool:
    return os.environ.get(var, "").lower() in {"1", "true", "yes"}


def _join(url: str, path: str) -> str:
    p = urlsplit(url)
    return urlunsplit((p.scheme, p.netloc, p.path.rstrip("/") + path, p.query, ""))


async def _exercise(sb: Sandbox, *, env_expected: bool) -> None:
    svc = sb.service("mcp")
    # Dropped headers would be a 400 from the server.
    bare = await svc.request("POST", "/mcp", json={"jsonrpc": "2.0", "id": 0, "method": "x"})
    assert bare.status_code == 400, bare.text
    init = await svc.request(
        "POST", "/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "initialize"}, headers=HEADERS
    )
    assert init.status_code == 200, init.text
    sid = init.headers["mcp-session-id"]
    call = await svc.request(
        "POST",
        "/mcp",
        json={
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "add", "arguments": {"a": 2, "b": 3}},
        },
        headers={**HEADERS, "mcp-session-id": sid},
    )
    assert call.json()["result"]["content"][0]["text"] == "5", call.text
    if env_expected:
        env = await svc.request(
            "POST",
            "/mcp",
            json={"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "env"}},
            headers={**HEADERS, "mcp-session-id": sid},
        )
        assert env.json()["result"]["content"][0]["text"] == "hello", env.text

    info = await sb.info()
    assert info.status == "ready" and info.services.get("mcp") == 8765, info

    async with httpx.AsyncClient(timeout=30) as client:
        url = await svc.url()
        assert (await client.get(_join(url, "/health"))).status_code == 200
        public = await sb.public_url("mcp", ttl=600)
        assert (await client.get(_join(public.url, "/health"))).status_code == 200
        r = await client.post(
            _join(public.url, "/mcp"),
            json={"jsonrpc": "2.0", "id": 1, "method": "initialize"},
            headers=HEADERS,
        )
        assert r.status_code == 200 and r.headers.get("mcp-session-id"), r.text
        async with sb.tunnel.forward(8765) as t:
            assert t.url.startswith("http://127.0.0.1:"), t.url
            assert (await client.get(t.url + "/health")).status_code == 200
            r = await client.post(
                t.url + "/mcp",
                json={"jsonrpc": "2.0", "id": 1, "method": "initialize"},
                headers=HEADERS,
            )
            assert r.status_code == 200, r.text


@pytest.mark.skipif(not _on("CUA_TEST_UNIFIED_LOCAL"), reason="CUA_TEST_UNIFIED_LOCAL is not set")
async def test_plain_image_mcp_local_gvisor(monkeypatch):
    cli = os.environ.get("CUA_BIN")
    if not cli:
        pytest.skip("CUA_BIN must name the cua CLI (the daemon serves local public URLs)")
    # A short throwaway home: the daemon's Unix socket path must fit SUN_LEN.
    home = tempfile.mkdtemp(prefix="cua-e2e-")
    monkeypatch.setenv("CUA_HOME", home)
    try:
        async with Sandbox.ephemeral(
            Image.from_registry("python:3.12-slim", kind="container"),
            command=["python", "-c", SERVER],
            env={"GREETING": "hello"},
            services={"mcp": 8765},
            wait_for=http("mcp", "/health"),
            local=True,
            name=f"cua-e2e-unified-{secrets.token_hex(3)}",
            telemetry_enabled=False,
        ) as sb:
            assert (await sb.info()).location == "local"
            await _exercise(sb, env_expected=True)
    finally:
        subprocess.run([cli, "daemon", "stop"], env={**os.environ, "CUA_HOME": home}, check=False)


LAYER_SERVER = r"""
import http.server, json, os, six
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({
            "six": six.__version__,
            "greeting": os.environ.get("LAYER_GREETING"),
            "file": open("/srv/greeting.txt").read(),
            "built": open("/built.txt").read(),
        }).encode()
        self.send_response(200); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("0.0.0.0", 8000), H).serve_forever()
"""


def _docker(*args: str) -> str:
    """The docker CLI on the current engine with a throwaway DOCKER_CONFIG
    (the user's is never read or written)."""
    host = (
        os.environ.get("DOCKER_HOST")
        or subprocess.run(
            ["docker", "context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip()
    )
    with tempfile.TemporaryDirectory(prefix="cua-e2e-docker-") as cfg:
        env = {**os.environ, "DOCKER_CONFIG": cfg, **({"DOCKER_HOST": host} if host else {})}
        return subprocess.run(
            ["docker", *args], capture_output=True, text=True, env=env, check=False
        ).stdout


def _built_tags() -> set[str]:
    return set(_docker("images", "cua-vmm/build", "--format", "{{.Tag}}").split())


@pytest.mark.skipif(not _on("CUA_TEST_UNIFIED_LOCAL"), reason="CUA_TEST_UNIFIED_LOCAL is not set")
async def test_plain_image_layers_build_locally(monkeypatch, tmp_path):
    """Image layers on a plain image (no cua-spacesd) with local=True:
    built into the container engine before boot, cached by content."""
    home = tempfile.mkdtemp(prefix="cua-e2e-")
    monkeypatch.setenv("CUA_HOME", home)
    greeting = tmp_path / "greeting.txt"
    greeting.write_text(f"hi {secrets.token_hex(4)}")
    image = (
        Image.from_registry("python:3.12-slim", kind="container")
        .pip_install("six==1.16.0")
        .run("echo built > /built.txt")
        .env(LAYER_GREETING="hola")
        .copy(str(greeting), "/srv/greeting.txt")
    )
    before = _built_tags()
    created: set[str] = set()
    try:
        for attempt in range(2):
            async with Sandbox.ephemeral(
                image,
                command=["python", "-c", LAYER_SERVER],
                services={"web": 8000},
                wait_for=http("web", "/"),
                local=True,
                memory="512MB",
                name=f"cua-e2e-layers-{secrets.token_hex(3)}",
                telemetry_enabled=False,
            ) as sb:
                r = await sb.service("web").request("GET", "/")
                assert r.status_code == 200, r.text
                assert r.json() == {
                    "six": "1.16.0",
                    "greeting": "hola",
                    "file": greeting.read_text(),
                    "built": "built\n",
                }, r.text
            created = _built_tags() - before
            # One build, reused by the second sandbox (same content hash).
            assert len(created) == 1 and next(iter(created)).startswith("cua-b-"), created
    finally:
        for tag in created:
            _docker("rmi", "-f", f"cua-vmm/build:{tag}")


@pytest.mark.skipif(not _on("CUA_TEST_UNIFIED_FLEET"), reason="CUA_TEST_UNIFIED_FLEET is not set")
async def test_plain_image_mcp_fleet_gvisor():
    if not (os.environ.get("CUA_CLIENT_ID") or os.environ.get("FLEETS_TOKEN")):
        pytest.skip("no Fleet credentials")
    sb = None
    pool = None
    try:
        sb = await Sandbox.create(
            Image.from_registry("docker.io/library/python:3.12-slim"),
            command=["python", "-c", SERVER],
            services={"mcp": 8765},
            wait_for=http("mcp", "/health"),
            name=f"cua-e2e-unified-{secrets.token_hex(3)}",
            time_to_start=900,
            telemetry_enabled=False,
            local=False,
        )
        pool = sb.pool_name
        info = await sb.info()
        assert info.location == "cloud" and info.expires_at, info
        await _exercise(sb, env_expected=False)
    finally:
        if sb is not None:
            await sb.destroy()
        if pool:
            report = await _autopool.gc_pools([pool])
            assert pool in report.pools_deleted or not report.errors, report
