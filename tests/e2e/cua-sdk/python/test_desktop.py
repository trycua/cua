"""The spacesd reference desktop (linux) as a sandbox.

* local-container: the image as a local container sandbox created by the
  SDK (gVisor when available): env smoke, suspend/resume.
* run-omarchy (guide, rewritten for the spacesd image): dimensions,
  screenshot, clipboard, click + keypress verified by the grid fixture,
  ``/mcp`` initialize on the env port.
* connect-with-viewer (guide): cua-spacesd's HTML5 viewer (``/viewer/`` on
  the ``env`` service, port 3211) through the SDK service API and a
  forward, and ``Sandbox.viewer_url()`` minting a ticketed viewer link.

container lane: local sandbox. fleet-env lane: the same checks on a Fleet
gVisor pool of ``$CUA_E2E_FLEET_ENV_IMAGE``.
"""

from __future__ import annotations

import asyncio
import json
import secrets
from dataclasses import dataclass

import e2e
import pytest

import cua


@dataclass
class Desktop:
    c: cua.Cua
    sb: cua.Sandbox
    token: str
    fleet: bool


@pytest.fixture(scope="module")
def local_desktop(tmp_path_factory):
    e2e.require_image(e2e.desktop_image())
    c = cua.embedded(state_dir=str(tmp_path_factory.mktemp("desk")), fleet_from_env=False)
    token = secrets.token_hex(16)
    sb = e2e.run_async(
        c.sandboxes().create(
            cua.SandboxCreateOptions(
                on="local",
                image=f"container:{e2e.desktop_image()}",
                name=e2e.name("desktop"),
                token=token,
                env={"CUA_ENV_TOKEN": token},
                cpus=2,
                memory_mb=2048,
                services={"env": 3211},
                wait_for=[cua.ReadinessProbe(port=3211, http_path="/viewer/")],
                ready_timeout_ms=300_000,
            )
        ),
        timeout=600,
    )
    try:
        yield Desktop(c, sb, token, fleet=False)
    finally:
        e2e.run_async(sb.delete(), timeout=120)


@pytest.fixture(scope="module")
def fleet_desktop(tmp_path_factory):
    """A Fleet gVisor pool of the spacesd image. Fleet pools have no env
    or secret field, so the env token is materialised by an entrypoint
    override (the image reads /etc/cua/env-token)."""
    image = __import__("os").environ["CUA_E2E_FLEET_ENV_IMAGE"]
    c = cua.embedded(state_dir=str(tmp_path_factory.mktemp("fleetdesk")))
    fleet = c.fleet()
    pool = e2e.name("fleet-desktop")
    token = secrets.token_hex(16)
    sb = None
    try:
        e2e.run_async(
            fleet.apply_pool(
                cua.FleetPoolSpec(
                    name=pool,
                    image=image,
                    runtime="gvisor",
                    cpu=2,
                    memory_mb=4096,
                    services={"env": 3211},
                    command=e2e.env_token_command(token),
                    ttl_seconds_after_created=7200,
                )
            ),
            timeout=300,
        )
        sb = e2e.run_async(
            c.sandboxes().create(
                cua.SandboxCreateOptions(
                    on="cloud",
                    pool=pool,
                    name=f"{pool}-c",
                    token=token,
                    ready_timeout_ms=1_200_000,
                )
            ),
            timeout=1500,
        )
        yield Desktop(c, sb, token, fleet=True)
    finally:
        if sb is not None:
            e2e.run_async(sb.delete(), timeout=300)
        try:
            e2e.run_async(fleet.delete_pool(pool), timeout=300)
        except cua.CuaError.NotFound:
            pass


# ------------------------------------------------------------ local-container


@pytest.mark.e2e("local-container", "container")
def test_local_container_desktop(local_desktop):
    d = local_desktop

    async def body():
        # A local container runs under gVisor when Docker has it (the CI
        # runner installs it), else runc; runtime_type names the backend.
        expected = "gvisor" if e2e.docker_has_runsc() else "container"
        assert d.sb.runtime_type() == expected, d.sb.runtime_type()
        env = await e2e.wait_env(d.sb)
        await e2e.env_smoke(env, desktop=True)
        await d.sb.suspend()
        assert (await d.sb.refresh()).status in (
            cua.SandboxStatus.SUSPENDED,
            cua.SandboxStatus.STOPPED,
        )
        await d.sb.resume()
        env = await e2e.wait_env(d.sb)
        assert (
            await env.run(cua.SpacesdCommand(program="echo", args=["back"]))
        ).stdout == b"back\n"
        runtime = e2e.docker(
            "inspect", "--format", "{{.HostConfig.Runtime}}", d.sb.name()
        ).stdout.strip()
        print("container runtime:", runtime)
        if e2e.docker_has_runsc():
            assert runtime == "runsc"

    e2e.run_async(body(), timeout=600)


# ------------------------------------------------------------ run-omarchy


async def _omarchy(d: Desktop) -> None:
    env = await e2e.wait_env(d.sb)
    result = await e2e.desktop_checks(env)
    print("desktop:", result)
    if d.fleet:
        await _mcp_fleet(d)
    else:
        fwd = await d.sb.forward(3211)
        try:
            init = await asyncio.to_thread(e2e.mcp_initialize, fwd.local_addr(), d.token)
            print("mcp serverInfo:", init["serverInfo"])
        finally:
            await fwd.close()


async def _mcp_fleet(d: Desktop) -> None:
    """/mcp through the Fleet gateway with Service.request headers: the env
    token rides in x-cua-env-authorization (the gateway strips
    `authorization` and adds the Fleet bearer and claim itself)."""
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "cua-e2e", "version": "0.1.0"},
            },
        }
    ).encode()
    headers = [
        cua.HttpHeader(name="content-type", value="application/json"),
        cua.HttpHeader(name="accept", value="application/json, text/event-stream"),
        cua.HttpHeader(name="x-cua-env-authorization", value=f"Bearer {d.token}"),
    ]
    resp = await d.sb.service("env").request("POST", "/mcp", body, 30_000, headers)
    assert resp.status == 200 and b"serverInfo" in resp.body, (resp.status, resp.body[:300])


@pytest.mark.e2e("run-omarchy", "container")
def test_run_omarchy_local(local_desktop):
    e2e.run_async(_omarchy(local_desktop), timeout=600)


@pytest.mark.e2e("run-omarchy", "container")
def test_typed_click_reaches_gtk(local_desktop):
    async def body():
        env = await e2e.wait_env(local_desktop.sb)
        await e2e.sh_ok(env, "cua-fixtures start grid")
        try:
            gx, gy = await e2e.window_origin(env, "CUA Fixture Grid")
            await env.click(float(gx + 5 * 80 + 40), float(gy + 1 * 80 + 40))

            async def pressed():
                return [
                    e
                    for e in await e2e.fixture_log(env, "grid")
                    if e.get("type") == "button_press" and e.get("cell") == [5, 1]
                ]

            await e2e.poll("typed click in cell [5,1]", pressed, attempts=10, delay=0.5)
        finally:
            await env.sh("cua-fixtures stop grid", 10_000)

    e2e.run_async(body(), timeout=300)


@pytest.mark.e2e("run-omarchy", "fleet-env")
def test_run_omarchy_fleet(fleet_desktop):
    e2e.run_async(_omarchy(fleet_desktop), timeout=900)


# ------------------------------------------------------------ connect-with-viewer


async def _viewer(d: Desktop) -> None:
    env = d.sb.service("env")
    resp = await e2e.poll(
        "viewer /viewer/",
        lambda: _ok(env.request("GET", "/viewer/", None, 30_000)),
        attempts=30,
        delay=2,
    )
    assert b"viewer.js" in resp.body
    link = await d.sb.viewer_url(cua.ViewerOptions(ttl_seconds=600, view_only=True))
    assert "/viewer/#ticket=" in link.url, link.url
    assert link.expires_at_unix > 0
    if d.fleet:
        return  # the live session needs a browser (guide step)
    page_url = link.url.split("#", 1)[0]
    status, body = await asyncio.to_thread(e2e.http_get, page_url)
    assert status == 200 and b"viewer.js" in body
    fwd = await d.sb.forward(3211)
    try:
        status, body = await asyncio.to_thread(e2e.http_get, f"http://{fwd.local_addr()}/viewer/")
        assert status == 200 and b"viewer.js" in body
    finally:
        await fwd.close()


async def _ok(fut):
    r = await fut
    return r if r.status == 200 else None


@pytest.mark.e2e("connect-with-viewer", "container")
def test_viewer_local(local_desktop):
    e2e.run_async(_viewer(local_desktop), timeout=300)


@pytest.mark.e2e("connect-with-viewer", "fleet-env")
def test_viewer_fleet(fleet_desktop):
    e2e.run_async(_viewer(fleet_desktop), timeout=600)
