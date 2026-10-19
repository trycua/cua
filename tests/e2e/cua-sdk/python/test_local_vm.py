"""local-qemu through the SDK's own local provider (``vm:<disk>``).

The reference disk boots under QEMU (hvf/kvm), readiness is a user HTTP probe
on the viewer page (``/viewer/`` on 3211), and the forward reaches it:
lifecycle and readiness assume no daemon API. The
SDK's token reaches the guest spacesd through the cloud-init seed every
local Linux VM gets; the Rust lane covers checkpoint and fork through
cua-vmm.
"""

from __future__ import annotations

import asyncio
import os
import secrets
from pathlib import Path

import e2e
import pytest

import cua


def _desktop_disk() -> Path:
    return Path(
        # CUA_E2E_DISK_CUA_DESKTOP_LINUX: the pre-rename name, read for one release.
        os.environ.get("CUA_E2E_DISK_LINUX")
        or os.environ.get(
            "CUA_E2E_DISK_CUA_DESKTOP_LINUX",
            str(Path.home() / ".cache" / "cua-images-e2e" / "linux" / e2e.HOST_ARCH / "disk.img"),
        )
    )


def _vm_opts(token: str) -> cua.SandboxCreateOptions:
    disk = _desktop_disk()
    if not disk.exists():
        pytest.skip(
            f"missing {disk}; libs/images/build.sh linux --tag e2e --outputs rootfs,disk --out ~/.cache/cua-images-e2e"
        )
    return cua.SandboxCreateOptions(
        on="local",
        image=f"vm:{disk}",
        name=e2e.name("vm"),
        token=token,
        env={"CUA_ENV_TOKEN": token},
        cpus=2,
        memory_mb=3072,
        ports=[3211],
        wait_for=[cua.ReadinessProbe(port=3211, http_path="/viewer/")],
        ready_timeout_ms=600_000,
    )


@pytest.mark.e2e("local-qemu", "qemu")
def test_local_qemu_lifecycle_probe_forward(local_cua):
    async def body():
        sb = await local_cua.sandboxes().create(_vm_opts(secrets.token_hex(16)))
        try:
            assert sb.runtime_type() == "qemu", sb.runtime_type()
            fwd = await sb.forward(3211)
            try:
                status, page = await asyncio.to_thread(
                    e2e.http_get, f"http://{fwd.local_addr()}/viewer/"
                )
                assert status == 200 and b"viewer.js" in page
            finally:
                await fwd.close()
            await sb.suspend()
            await sb.resume()
            await sb.wait_ready([cua.ReadinessProbe(port=3211, http_path="/viewer/")], 120_000)
        finally:
            await sb.delete()

    e2e.run_async(body(), timeout=1200)


@pytest.mark.e2e("local-qemu", "qemu")
def test_local_qemu_env_token_reaches_guest(local_cua):
    async def body():
        token = secrets.token_hex(16)
        opts = _vm_opts(token)
        opts.ports = [3211]
        sb = await local_cua.sandboxes().create(opts)
        try:
            env = await e2e.wait_env(sb, 180)
            await e2e.env_smoke(env, desktop=True)
        finally:
            await sb.delete()

    e2e.run_async(body(), timeout=1200)
