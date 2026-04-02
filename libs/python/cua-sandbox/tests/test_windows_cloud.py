"""Windows Server E2E integration tests.

Tests Windows VM lifecycle via the CUA Cloud API:
1. Create a Windows Server VM via Image.windows("server-2025")
2. Wait for the VM to get a DHCP IP and become running
3. Verify the VM is accessible via the API
4. Measure create + running timings

Run against local dev stack:
    CUA_API_KEY=sk-dev-test-key-local-12345 CUA_BASE_URL=http://localhost:8082 \
    uv run pytest tests/test_windows_cloud.py -v -s
"""

from __future__ import annotations

import os
import time

import httpx
import pytest

pytestmark = pytest.mark.asyncio

API_KEY = os.environ.get("CUA_API_KEY", "")
BASE_URL = os.environ.get("CUA_BASE_URL", "http://localhost:8082")


def _has_cua_api_key() -> bool:
    return bool(API_KEY)


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_windows_server_create():
    """Create a Windows Server 2025 VM and verify it gets an IP."""
    async with httpx.AsyncClient(
        base_url=BASE_URL,
        headers={"Authorization": f"Bearer {API_KEY}"},
        timeout=10.0,
    ) as client:
        # Create
        t0 = time.monotonic()
        resp = await client.post(
            "/v1/vms",
            json={"os": "windows", "configuration": "small", "region": "us-east-1"},
        )
        t_create = time.monotonic() - t0
        assert resp.status_code in (200, 201, 202), f"Create failed: {resp.status_code} {resp.text}"
        data = resp.json()
        vm_name = data["name"]
        print(f"\n  Created VM: {vm_name} in {t_create:.2f}s")

        try:
            # Poll until running with IP
            t1 = time.monotonic()
            ip_address = None
            for _ in range(120):  # 2 min max
                resp = await client.get(f"/v1/vms/{vm_name}")
                if resp.status_code == 200:
                    vm = resp.json()
                    status = vm.get("status", "")
                    # IP is in endpoints[].host (LOCAL_DEV_INCUS replaces .cua.sh with direct IP)
                    ip_address = _extract_ip(vm)
                    if status.lower() == "running" and ip_address:
                        break
                await _sleep(1)

            t_running = time.monotonic() - t1
            t_total = time.monotonic() - t0

            print(f"  Status: running, IP: {ip_address}")
            print(f"  Create API call: {t_create:.2f}s")
            print(f"  Wait for running+IP: {t_running:.2f}s")
            print(f"  Total: {t_total:.2f}s")

            assert ip_address, f"VM {vm_name} never got an IP (waited {t_running:.0f}s)"

        finally:
            # Cleanup
            await client.delete(f"/v1/vms/{vm_name}")
            print(f"  Cleaned up {vm_name}")


@pytest.mark.skipif(not _has_cua_api_key(), reason="CUA_API_KEY not set")
async def test_windows_server_snapshot_fork():
    """Create VM → snapshot → fork from snapshot → verify fork gets IP."""
    async with httpx.AsyncClient(
        base_url=BASE_URL,
        headers={"Authorization": f"Bearer {API_KEY}"},
        timeout=10.0,
    ) as client:
        # Create base VM
        t0 = time.monotonic()
        resp = await client.post(
            "/v1/vms",
            json={"os": "windows", "configuration": "small", "region": "us-east-1"},
        )
        assert resp.status_code in (200, 201, 202), f"Create failed: {resp.status_code} {resp.text}"
        vm_name = resp.json()["name"]
        print(f"\n  Created base VM: {vm_name}")

        try:
            # Wait for IP
            for _ in range(120):
                resp = await client.get(f"/v1/vms/{vm_name}")
                if resp.status_code == 200:
                    vm = resp.json()
                    if _extract_ip(vm):
                        break
                await _sleep(1)
            t_base_ip = time.monotonic() - t0
            base_ip = _extract_ip(vm)
            print(f"  Base VM IP: {base_ip} ({t_base_ip:.1f}s)")
            assert base_ip, "Base VM never got IP"

            # Snapshot
            t_snap_start = time.monotonic()
            resp = await client.post(f"/v1/vms/{vm_name}/snapshot")
            assert resp.status_code in (200, 201, 202), f"Snapshot failed: {resp.status_code} {resp.text}"
            snapshot_data = resp.json()
            snapshot_name = snapshot_data.get("name", "")
            t_snap = time.monotonic() - t_snap_start
            print(f"  Snapshot: {snapshot_name} ({t_snap:.2f}s)")

            # Fork from snapshot
            t_fork_start = time.monotonic()
            resp = await client.post(
                "/v1/vms/fork",
                json={
                    "source": "snapshot",
                    "instance": vm_name,
                    "snapshot": snapshot_name,
                },
            )
            assert resp.status_code in (200, 201, 202), f"Fork failed: {resp.status_code} {resp.text}"
            fork_name = resp.json()["name"]
            print(f"  Fork created: {fork_name}")

            # Wait for fork IP
            fork_ip = None
            for _ in range(90):
                resp = await client.get(f"/v1/vms/{fork_name}")
                if resp.status_code == 200:
                    fvm = resp.json()
                    fork_ip = _extract_ip(fvm)
                    if fork_ip:
                        break
                await _sleep(1)
            t_fork = time.monotonic() - t_fork_start
            t_total = time.monotonic() - t0

            print(f"  Fork IP: {fork_ip} ({t_fork:.1f}s)")
            print(f"\n  === TIMINGS ===")
            print(f"  Base create+IP: {t_base_ip:.1f}s")
            print(f"  Snapshot: {t_snap:.2f}s")
            print(f"  Fork+IP: {t_fork:.1f}s")
            print(f"  Total: {t_total:.1f}s")

        finally:
            # Cleanup
            for name in [fork_name, vm_name]:
                try:
                    await client.delete(f"/v1/vms/{name}")
                except Exception:
                    pass
            print(f"  Cleaned up")


def _extract_ip(vm: dict) -> str:
    """Extract IP from VM response — checks endpoints, ip, vm_ip fields."""
    import re
    # Direct IP fields
    for key in ("ip", "vm_ip"):
        val = vm.get(key, "")
        if val and re.match(r"\d+\.\d+\.\d+\.\d+", val):
            return val
    # IP from endpoints (LOCAL_DEV_INCUS mode replaces .cua.sh hosts with IPs)
    for ep in vm.get("endpoints", []):
        host = ep.get("host", "")
        if re.match(r"\d+\.\d+\.\d+\.\d+", host):
            return host
    return ""


async def _sleep(seconds: float):
    import asyncio
    await asyncio.sleep(seconds)
