"""daemon-vs-embedded: the same script runs embedded and against a
``cua daemon`` with identical results, and two processes share one
daemon-held sandbox.

hermetic: the target is the MockServer spacesd (direct provider).
container: the target is a local linux container sandbox
(gVisor when available) created *by* the SDK in each topology.
"""

from __future__ import annotations

import os
import secrets
import subprocess
import sys
import textwrap

import e2e
import pytest

import cua


def _local_opts(name: str, token: str) -> cua.SandboxCreateOptions:
    return cua.SandboxCreateOptions(
        on="local",
        image=f"container:{e2e.desktop_image()}",
        name=name,
        token=token,
        env={"CUA_ENV_TOKEN": token},
        cpus=2,
        memory_mb=2048,
        wait_for=[cua.ReadinessProbe(port=3211)],
        ready_timeout_ms=300_000,
    )


async def script(c: cua.Cua, target: dict, suffix: str) -> dict:
    """The one script both topologies run. Returns a topology-independent summary."""
    sbx = c.sandboxes()
    name = e2e.name(f"dve-{suffix}")
    if target["kind"] == "direct":
        sb = await sbx.connect_url(target["url"], target["token"], name)
    else:
        sb = await sbx.create(_local_opts(name, target["token"]))
    try:
        env = await e2e.wait_env(sb)
        summary = await e2e.env_smoke(env, desktop=True, mock=target.get("mock", False))
        info = await sbx.get(name)
        summary["sandbox"] = (
            info.location,
            info.runtime_type,
            info.ephemeral,
            info.status.name,
        )
        summary["listed"] = name in [s.name for s in await sbx.list(None)]
        summary["services"] = sorted(sb.services())
        return summary
    finally:
        await sb.delete()


PEER = textwrap.dedent("""
    import asyncio, sys, cua
    sock, name, kind, url, token, image, marker = sys.argv[1:8]
    async def main():
        c = cua.connect(sock)
        sbx = c.sandboxes()
        if kind == "direct":
            sb = await sbx.connect_url(url, token, name)
        else:
            sb = await sbx.create(cua.SandboxCreateOptions(
                on="local", image="container:" + image, name=name,
                token=token, env={"CUA_ENV_TOKEN": token}, cpus=2, memory_mb=2048,
                wait_for=[cua.ReadinessProbe(port=3211)], ready_timeout_ms=300_000))
        for _ in range(120):
            try:
                env = await sb.spacesd(5000)
                break
            except cua.CuaError.SpacesdNotAvailable:
                await asyncio.sleep(1)
        await env.upload("/tmp/cua-e2e-shared-marker", marker.encode(), None)
        print("pid", (await c.info()).daemon_pid, flush=True)
    asyncio.run(asyncio.wait_for(main(), 600))
    """)


def _shared_sandbox(daemon: e2e.Daemon, target: dict) -> None:
    """Process A (a subprocess) creates the sandbox through the daemon and
    leaves it running; process B (this test) reattaches by name and sees
    A's state. Only one sandbox exists."""
    name = e2e.name("dve-shared")
    marker = secrets.token_hex(8)
    peer = subprocess.run(
        [
            sys.executable,
            "-c",
            PEER,
            str(daemon.socket),
            name,
            target["kind"],
            target.get("url", ""),
            target["token"],
            e2e.desktop_image(),
            marker,
        ],
        capture_output=True,
        text=True,
        timeout=900,
        env=dict(os.environ, PYTHONPATH=os.pathsep.join(p for p in sys.path if p)),
    )
    assert peer.returncode == 0, peer.stderr[-2000:]

    async def body():
        c = daemon.client()
        assert f"pid {(await c.info()).daemon_pid}" in peer.stdout
        sbx = c.sandboxes()
        assert [s.name for s in await sbx.list(None)].count(name) == 1
        sb = await sbx.connect(name)
        try:
            env = await sb.spacesd(10_000)
            assert (await env.download("/tmp/cua-e2e-shared-marker")).decode() == marker
            if target["kind"] == "local":
                ps = e2e.docker(
                    "ps", "--filter", f"name=^{name}$", "--format", "{{.Names}}"
                ).stdout.split()
                assert ps == [name], ps
        finally:
            await sb.delete()

    e2e.run_async(body(), timeout=300)


def _compare(target: dict, tmp_path) -> None:
    embedded = e2e.run_async(
        script(cua.embedded(state_dir=str(tmp_path / "emb"), fleet_from_env=False), target, "emb"),
        timeout=900,
    )
    with e2e.cua_daemon() as d:
        c = d.client()
        assert c.mode() == cua.CuaMode.DAEMON
        via_daemon = e2e.run_async(script(c, target, "dmn"), timeout=900)
        print("embedded:", embedded)
        print("daemon:  ", via_daemon)
        assert via_daemon == embedded
        _shared_sandbox(d, target)


@pytest.mark.e2e("daemon-vs-embedded", "hermetic")
def test_daemon_vs_embedded_mock(fixtures, tmp_path):
    _compare(
        {
            "kind": "direct",
            "url": fixtures["env_url"],
            "token": fixtures["env_token"],
            "mock": True,
        },
        tmp_path,
    )


@pytest.mark.e2e("daemon-vs-embedded", "container")
def test_daemon_vs_embedded_local_container(tmp_path):
    e2e.require_image(e2e.desktop_image())
    _compare({"kind": "local", "token": secrets.token_hex(16)}, tmp_path)
