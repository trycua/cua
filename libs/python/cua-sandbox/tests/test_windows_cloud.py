"""Windows Server E2E test using cua-sandbox SDK internals.

Tests the cloud transport VM lifecycle (create, wait for running, snapshot,
fork) without requiring computer-server (which isn't yet installed in the
Windows golden image).

Once computer-server is added to the golden image, this test should be
upgraded to use Sandbox.ephemeral() with full screenshot verification.

Run:
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    uv run pytest tests/test_windows_cloud.py -v -s
"""

import logging
import os
import time

import httpx
import pytest

pytestmark = pytest.mark.asyncio

logging.basicConfig(level=logging.DEBUG)
logger = logging.getLogger(__name__)

API_KEY = os.environ.get("CUA_API_KEY", "")
BASE_URL = os.environ.get("CUA_BASE_URL", "http://localhost:8082")


def _has_env() -> bool:
    return bool(API_KEY)


def _extract_ip(vm: dict) -> str:
    """Extract IP from VM response."""
    import re
    for key in ("ip", "vm_ip"):
        val = vm.get(key, "")
        if val and re.match(r"\d+\.\d+\.\d+\.\d+", val):
            return val
    for ep in vm.get("endpoints", []):
        host = ep.get("host", "")
        if re.match(r"\d+\.\d+\.\d+\.\d+", host):
            return host
    return ""


@pytest.mark.skipif(not _has_env(), reason="CUA_API_KEY not set")
async def test_windows_create_snapshot_fork():
    """Create Windows VM → wait for IP → snapshot → fork → verify fork IP.

    Uses the same API calls that Sandbox.ephemeral() makes internally
    (CloudTransport._create_vm, _wait_for_running, snapshot, fork).
    """
    import asyncio

    async with httpx.AsyncClient(
        base_url=BASE_URL,
        headers={"Authorization": f"Bearer {API_KEY}"},
        timeout=30.0,
    ) as client:

        # ── Step 1: Create VM (same as CloudTransport._create_vm) ──
        t0 = time.monotonic()
        resp = await client.post(
            "/v1/vms",
            json={"os": "windows", "configuration": "small", "region": "us-east-1"},
        )
        t_api = time.monotonic() - t0
        assert resp.status_code in (200, 201, 202), f"Create failed: {resp.status_code} {resp.text}"
        vm_name = resp.json()["name"]
        logger.info(f"Created VM {vm_name} in {t_api:.2f}s")

        try:
            # ── Step 2: Wait for running + IP (same as CloudTransport._wait_for_running) ──
            ip = None
            for i in range(120):
                resp = await client.get(f"/v1/vms/{vm_name}")
                if resp.status_code == 200:
                    vm = resp.json()
                    if vm.get("status", "").lower() == "running":
                        ip = _extract_ip(vm)
                        if ip:
                            break
                await asyncio.sleep(0.5)

            t_running = time.monotonic() - t0
            assert ip, f"VM {vm_name} never got IP (waited {t_running:.0f}s)"
            logger.info(f"VM running with IP {ip} in {t_running:.1f}s")

            # ── Step 3: Snapshot (same as Sandbox.snapshot) ──
            t_snap = time.monotonic()
            resp = await client.post(f"/v1/vms/{vm_name}/snapshot")
            assert resp.status_code in (200, 201, 202), f"Snapshot failed: {resp.status_code} {resp.text}"
            snap = resp.json()
            snap_name = snap.get("name", "")
            t_snap_dur = time.monotonic() - t_snap
            logger.info(f"Snapshot '{snap_name}' in {t_snap_dur:.2f}s")

            # ── Step 4: Fork from snapshot (same as Sandbox.ephemeral(snapshot_img)) ──
            t_fork = time.monotonic()
            resp = await client.post(
                "/v1/vms/fork",
                json={"source": "snapshot", "instance": vm_name, "snapshot": snap_name},
            )
            assert resp.status_code in (200, 201, 202), f"Fork failed: {resp.status_code} {resp.text}"
            fork_name = resp.json()["name"]
            logger.info(f"Fork {fork_name} created")

            # Wait for fork IP
            fork_ip = None
            for i in range(120):
                resp = await client.get(f"/v1/vms/{fork_name}")
                if resp.status_code == 200:
                    fvm = resp.json()
                    if fvm.get("status", "").lower() == "running":
                        fork_ip = _extract_ip(fvm)
                        if fork_ip:
                            break
                await asyncio.sleep(0.5)

            t_fork_dur = time.monotonic() - t_fork
            t_total = time.monotonic() - t0

            # ── Results ──
            print(f"\n{'='*50}")
            print(f"  VM create API:     {t_api:.2f}s")
            print(f"  VM running + IP:   {t_running:.1f}s  ({ip})")
            print(f"  Snapshot:          {t_snap_dur:.2f}s  ({snap_name})")
            print(f"  Fork + IP:         {t_fork_dur:.1f}s  ({fork_ip})")
            print(f"  TOTAL:             {t_total:.1f}s")
            print(f"{'='*50}")

            assert fork_ip, f"Fork {fork_name} never got IP (waited {t_fork_dur:.0f}s)"

        finally:
            # Cleanup both VMs
            for name in [locals().get("fork_name"), vm_name]:
                if name:
                    try:
                        await client.delete(f"/v1/vms/{name}")
                        logger.info(f"Cleaned up {name}")
                    except Exception:
                        pass
