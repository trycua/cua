"""Local container (gVisor) sandbox through the Python binding. Gated:
CUA_E2E_LOCAL=1 and a Docker-API engine (Colima) with `runsc`. Starts a
small `cua-e2e-*` nginx container, reaches it through `forward` and a
service request, and deletes it in `finally`."""

from __future__ import annotations

import asyncio
import json
import os
import secrets
import shutil
import subprocess
import urllib.request

import pytest

import cua

IMAGE = os.environ.get("CUA_E2E_LOCAL_IMAGE", "docker.io/library/nginx:1.27-alpine")

pytestmark = pytest.mark.skipif(
    os.environ.get("CUA_E2E_LOCAL") != "1", reason="set CUA_E2E_LOCAL=1 for local container e2e"
)


def test_gvisor_container_forward_and_probe(tmp_path):
    name = f"cua-e2e-py-local-{secrets.token_hex(3)}"

    async def body():
        c = cua.embedded(state_dir=str(tmp_path), fleet_from_env=False)
        report = await c.local().doctor()
        container = next(ch for ch in report.checks if ch.name == "container")
        assert container.status == cua.RuntimeCheckStatus.OK, container
        sb = None
        try:
            sb = await c.sandboxes().create(
                cua.SandboxCreateOptions(
                    on="local",
                    kind="container",
                    image=IMAGE,
                    name=name,
                    cpus=1,
                    memory_mb=512,
                    ports=[80],
                    services={"web": 80},
                    wait_for=[cua.ReadinessProbe(port=80, http_path="/")],
                    ready_timeout_ms=300_000,
                )
            )
            assert sb.location() == "local"
            assert sb.kind() == "container"

            fwd = await sb.forward(80)
            addr = fwd.local_addr()
            assert addr
            body = await asyncio.to_thread(
                lambda: urllib.request.urlopen(f"http://{addr}/", timeout=10).read()
            )
            assert b"nginx" in body.lower()
            await fwd.close()

            resp = await sb.service("web").request("GET", "/", None, 10_000)
            assert resp.status == 200

            if shutil.which("docker"):
                runtime = subprocess.run(
                    ["docker", "inspect", "--format", "{{json .HostConfig.Runtime}}", name],
                    capture_output=True,
                    text=True,
                    timeout=30,
                ).stdout.strip()
                print("container runtime:", runtime)
                assert json.loads(runtime) == "runsc"
        finally:
            if sb is not None:
                await sb.delete()
        names = [s.name for s in await c.sandboxes().list("local")]
        assert name not in names

    asyncio.run(asyncio.wait_for(body(), timeout=600))
