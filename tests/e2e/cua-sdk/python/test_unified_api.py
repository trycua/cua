"""unified-sandbox-api: the same sandbox calls local and in the cloud.

A plain image running its own server as ``command``, reached as a named
service (``services``) after a readiness probe on it (``wait_for``):

* ``service(name).request`` with caller headers (the MCP-style server
  answers 400 when ``accept`` / ``mcp-session-id`` are dropped),
* ``service(name).url()`` and ``public_url(name, ttl)`` used by a plain HTTP
  client with no credentials (local: a token URL served by the cua daemon;
  cloud: a signed URL),
* ``forward(port)`` as a loopback URL (cloud images without cua-spacesd:
  a proxy through the gateway),
* portable info: ``id``, phase, location, ``provider_details``.

Lanes: hermetic (fake Fleet + MockServer through a private daemon),
container (``python:3.12-slim`` on gVisor through a private daemon), fleet
(live, one sandbox, managed pool removed in ``finally``).
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from urllib.parse import urlsplit, urlunsplit

import e2e
import pytest

import cua

SERVER = (e2e.SUITE / "fixtures" / "mcp_probe_server.py").read_text()
ACCEPT = "application/json, text/event-stream"


def _join(url: str, path: str) -> str:
    p = urlsplit(url)
    return urlunsplit((p.scheme, p.netloc, p.path.rstrip("/") + path, p.query, ""))


def _http(method: str, url: str, body: dict | None = None, headers: dict | None = None):
    """A plain HTTP client with no SDK credentials: (status, headers, body)."""
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    if data is not None:
        req.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as err:
        return err.code, dict(err.headers), err.read()


def _h(name: str, value: str) -> "cua.HttpHeader":
    return cua.HttpHeader(name=name, value=value)


async def _mcp_through_service(sb: "cua.Sandbox", *, env_expected: bool) -> None:
    svc = sb.service("mcp")
    body = lambda m, i, p=None: json.dumps(  # noqa: E731
        {"jsonrpc": "2.0", "id": i, "method": m, **({"params": p} if p else {})}
    ).encode()
    ct = _h("content-type", "application/json")
    bare = await svc.request("POST", "/mcp", body("x", 0), 30_000, [ct])
    assert bare.status == 400, bytes(bare.body)
    init = await svc.request(
        "POST", "/mcp", body("initialize", 1), 30_000, [ct, _h("accept", ACCEPT)]
    )
    assert init.status == 200, bytes(init.body)
    sid = next(h.value for h in init.headers if h.name.lower() == "mcp-session-id")
    heads = [ct, _h("accept", ACCEPT), _h("mcp-session-id", sid)]
    call = await svc.request(
        "POST",
        "/mcp",
        body("tools/call", 2, {"name": "add", "arguments": {"a": 2, "b": 3}}),
        30_000,
        heads,
    )
    assert json.loads(bytes(call.body))["result"]["content"][0]["text"] == "5"
    if env_expected:
        env = await svc.request(
            "POST", "/mcp", body("tools/call", 3, {"name": "env"}), 30_000, heads
        )
        assert json.loads(bytes(env.body))["result"]["content"][0]["text"] == "hello"


def _plain_http_checks(url: str) -> None:
    status, _, text = _http("GET", _join(url, "/health"))
    assert status == 200, text
    status, headers, text = _http(
        "POST",
        _join(url, "/mcp"),
        {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
        {"accept": ACCEPT},
    )
    assert status == 200 and {k.lower() for k in headers} >= {"mcp-session-id"}, text


async def _urls_and_forward(sb: "cua.Sandbox", port: int) -> None:
    _plain_http_checks(await sb.service("mcp").url())
    public = await sb.public_url("mcp", 600, "e2e")
    assert public.expires_at_unix > 0
    _plain_http_checks(public.url)
    fwd = await sb.forward(port)
    try:
        url = fwd.url()
        assert url and url.startswith("http://127.0.0.1:"), url
        _plain_http_checks(url)
    finally:
        await fwd.close()
    await sb.revoke_public_url(public.id)


def _plain_opts(image: str, what: str, *, env: bool) -> "cua.SandboxCreateOptions":
    return cua.SandboxCreateOptions(
        on="local",
        image=image,
        name=e2e.name(what),
        cpus=1,
        memory_mb=512,
        command=["python", "-c", SERVER],
        env={"GREETING": "hello"} if env else {},
        services={"mcp": 8765},
        wait_for=[cua.ReadinessProbe(port=0, service="mcp", http_path="/health")],
        ready_timeout_ms=300_000,
    )


@pytest.mark.e2e("unified-sandbox-api", "hermetic")
def test_unified_api_fake_fleet_and_daemon_public_url(fixtures, fake_fleet):
    async def body():
        # Cloud (fake Fleet): command and services on a managed gVisor pool;
        # signed URLs; the forward proxies through the gateway.
        sbx = fake_fleet.sandboxes()
        sb = await sbx.create(
            cua.SandboxCreateOptions(
                on="cloud",
                image="registry.example/mcp:docker-e2e",
                command=["python", "/srv.py"],
                services={"mcp": 8765},
                runtime="gvisor",
                cloud=cua.CloudOptions(max_pool_size=2),
                ready_timeout_ms=120_000,
            )
        )
        pool = sb.info().provider_details.get("pool", "")
        try:
            info = sb.info()
            assert (info.location, info.phase) == ("cloud", cua.SandboxPhase.READY), info
            assert info.id and pool.startswith("cua-auto-"), info
            assert (await sb.service("mcp").url()).startswith("https://signed.fleet.test/")
            p = await sb.public_url("mcp", 600, None)
            assert p.url.startswith("https://signed.fleet.test/") and "claim" in p.provider_details
            r = await sb.service("mcp").request(
                "POST", "/mcp", b"{}", 30_000, [_h("accept", ACCEPT), _h("mcp-session-id", "s")]
            )
            assert r.status == 200
            fwd = await sb.forward(8765)
            try:
                status, _, text = _http("GET", fwd.url() + "/x")
                assert status == 200 and b"-mcp/x" in text, text
            finally:
                await fwd.close()
        finally:
            await sb.delete()
            if pool:
                await fake_fleet.fleet().pools().gc_pools([pool], 0)

        # Local public URL (a direct spacesd): served by the daemon; any
        # gRPC-Web client reaches the driver through it with its own token.
        with e2e.cua_daemon() as d:
            c = d.client()
            direct = await c.sandboxes().connect_url(
                fixtures["env_url"], fixtures["env_token"], None
            )
            assert (await direct.service("env").url()) == fixtures["env_url"].rstrip("/")
            pub = await direct.public_url("env", 120, None)
            assert pub.url.startswith("http://127.0.0.1:") and "/s/" in pub.url, pub
            shared = await fake_fleet.sandboxes().connect_url(pub.url, fixtures["env_token"], None)
            out = await (await shared.spacesd(5_000)).sh("echo shared", None)
            assert bytes(out.stdout) == b"shared\n"
            await direct.revoke_public_url(pub.id)
            status, _, _ = _http("GET", pub.url)
            assert status == 404

    e2e.run_async(body(), 300)


@pytest.mark.e2e("unified-sandbox-api", "container")
def test_unified_api_plain_image_gvisor():
    if not e2e.docker_image_exists("python:3.12-slim"):
        e2e.docker("pull", "python:3.12-slim")

    async def body():
        with e2e.cua_daemon() as d:
            c = d.client()
            sb = await c.sandboxes().create(
                _plain_opts("container:python:3.12-slim", "unified", env=True)
            )
            try:
                info = sb.info()
                assert (info.location, info.phase) == ("local", cua.SandboxPhase.READY), info
                assert info.services.get("mcp") == 8765
                assert "backend" in info.provider_details
                await _mcp_through_service(sb, env_expected=True)
                await _urls_and_forward(sb, 8765)
            finally:
                await sb.delete()

    e2e.run_async(body(), 600)


@pytest.mark.e2e("unified-sandbox-api", "fleet")
def test_unified_api_plain_image_fleet(live_fleet):
    async def body():
        o = _plain_opts("docker.io/library/python:3.12-slim", "unified", env=False)
        o.on = "cloud"
        o.ready_timeout_ms = 1_200_000
        sb = await live_fleet.sandboxes().create(o)
        pool = sb.info().provider_details.get("pool", "")
        try:
            info = sb.info()
            assert (info.location, info.phase) == ("cloud", cua.SandboxPhase.READY), info
            assert info.expires_at_unix, info
            await _mcp_through_service(sb, env_expected=False)
            await _urls_and_forward(sb, 8765)
        finally:
            await sb.delete()
            if pool:
                await live_fleet.fleet().pools().gc_pools([pool], 0)

    e2e.run_async(body(), 1500)
