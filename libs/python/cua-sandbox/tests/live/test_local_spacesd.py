"""Live: a local container sandbox with cua-spacesd, through the cua SDK.

Gated on ``CUA_TEST_LOCAL_ENV_IMAGE`` (a linux rootfs tag, e.g.
``cua-e2e-local/linux:docker-local-arm64`` built with
``libs/images/build.sh linux``) and a Docker engine. The SDK runs
the container under gVisor (``runsc``) when the engine has it, with its
default 4 GiB memory cap. Every interface family is exercised, then the
container must be gone.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import uuid

import pytest
from cua_sandbox import Image, Sandbox
from cua_sandbox.runtime import DockerRuntime

IMAGE = os.environ.get("CUA_TEST_LOCAL_ENV_IMAGE")

pytestmark = pytest.mark.skipif(not IMAGE, reason="CUA_TEST_LOCAL_ENV_IMAGE is not set")


def _docker(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["docker", *args], capture_output=True, text=True, timeout=60)


async def _poll(check, what: str, attempts: int = 40, delay: float = 0.5):
    for _ in range(attempts):  # bounded: never loops forever
        value = await check()
        if value:
            return value
        await asyncio.sleep(delay)
    raise AssertionError(f"timed out waiting for {what}")


async def test_local_container_sandbox_through_spacesd(tmp_path):
    name = f"cua-e2e-sbx-{uuid.uuid4().hex[:8]}"
    image = Image.from_registry(IMAGE, os_type="linux", kind="container")
    runtime = DockerRuntime(memory_mb=2048, cpus=2)

    async with Sandbox.ephemeral(
        image, local=True, runtime=runtime, name=name, telemetry_enabled=False
    ) as sb:
        inspect = json.loads(_docker("inspect", name).stdout)[0]
        host_config = inspect["HostConfig"]
        print("container runtime:", host_config.get("Runtime"), "memory:", host_config["Memory"])
        assert host_config["Memory"] == 2048 * 1024 * 1024
        if "runsc" in _docker("info", "--format", "{{json .Runtimes}}").stdout:
            assert host_config.get("Runtime") == "runsc"

        # shell
        uname = await sb.shell.run("uname -a")
        assert uname.success and "Linux" in uname.stdout
        failed = await sb.shell.run("exit 7")
        assert failed.returncode == 7

        # files round trip (binary, text, upload/download, listing)
        payload = os.urandom(300_000)
        await sb.files.write_bytes("/tmp/cua-e2e/blob.bin", payload)
        assert await sb.files.read_bytes("/tmp/cua-e2e/blob.bin") == payload
        await sb.files.write_text("/tmp/cua-e2e/note.txt", "héllo from cua-sandbox")
        assert await sb.files.read_text("/tmp/cua-e2e/note.txt") == "héllo from cua-sandbox"
        assert "note.txt" in {entry.name for entry in await sb.files.list("/tmp/cua-e2e")}
        local = tmp_path / "down.bin"
        await sb.files.download("/tmp/cua-e2e/blob.bin", local)
        assert local.read_bytes() == payload
        check = await sb.shell.run("sha256sum /tmp/cua-e2e/blob.bin | cut -c1-64")
        import hashlib

        assert check.stdout.strip() == hashlib.sha256(payload).hexdigest()

        # screen
        png = await sb.screenshot()
        assert png.startswith(b"\x89PNG") and len(png) > 10_000
        (tmp_path / "desktop.png").write_bytes(png)
        width, height = await sb.get_dimensions()
        assert width > 0 and height > 0
        assert await sb.get_environment() == "linux"

        # window title: open a titled terminal, focus it, read the title back
        title = f"cua-e2e-{uuid.uuid4().hex[:6]}"
        await sb.shell.run(
            f"DISPLAY=:1 nohup xfce4-terminal --title={title} >/dev/null 2>&1 &", timeout=10
        )

        async def focused_title():
            await sb.shell.run(
                f"DISPLAY=:1 xdotool search --name {title} windowactivate --sync", timeout=10
            )
            got = await sb.window.get_active_title()
            return got if title in got else None

        assert title in await _poll(focused_title, "the terminal to take focus")

        # keyboard + mouse: type into the focused terminal and prove it ran
        marker = f"/tmp/cua-e2e/typed-{uuid.uuid4().hex[:6]}"
        await sb.mouse.move(width // 2, height // 2)
        await sb.keyboard.type(f"touch {marker}")
        await sb.keyboard.keypress("enter")

        async def marker_exists():
            return await sb.files.exists(marker)

        await _poll(marker_exists, "the typed command to run")
        await sb.mouse.click(width // 2, height // 2)
        await sb.mouse.scroll(width // 2, height // 2, scroll_y=-3)
        await sb.keyboard.keypress(["ctrl", "l"])

        # clipboard
        await sb.clipboard.set("cua-e2e clipboard")
        assert await sb.clipboard.get() == "cua-e2e clipboard"

        # raw env client
        env = await sb.spacesd()
        caps = await env.capabilities()
        assert caps.os_family.lower() == "linux"

    assert _docker("inspect", "--type", "container", name).returncode != 0, "container leaked"


async def test_persistent_local_sandbox_lifecycle():
    """create → reconnect by name (token from state) → suspend → resume → delete."""
    name = f"cua-e2e-sbx-{uuid.uuid4().hex[:8]}"
    image = Image.from_registry(IMAGE, os_type="linux", kind="container")
    sb = await Sandbox.create(
        image,
        local=True,
        runtime=DockerRuntime(ephemeral=False, memory_mb=2048),
        name=name,
        telemetry_enabled=False,
    )
    try:
        assert (await sb.shell.run("echo first")).stdout == "first\n"
        await sb.disconnect()

        info = await Sandbox.get_info(name, local=True)
        assert info.status == "running" and info.source == "container"
        assert name in {s.name for s in await Sandbox.list(local=True)}

        again = await Sandbox.connect(name, local=True)
        assert (await again.shell.run("echo again")).stdout == "again\n"
        await again.disconnect()

        await Sandbox.suspend(name, local=True)
        resumed = await Sandbox.resume(name, local=True)
        assert (await resumed.shell.run("echo resumed")).stdout == "resumed\n"
        await resumed.disconnect()
    finally:
        await Sandbox.delete(name, local=True)
    assert _docker("inspect", "--type", "container", name).returncode != 0, "container leaked"
    assert name not in {s.name for s in await Sandbox.list(local=True)}


PLAIN_IMAGE = os.environ.get("CUA_TEST_LOCAL_PLAIN_IMAGE")


@pytest.mark.skipif(not PLAIN_IMAGE, reason="CUA_TEST_LOCAL_PLAIN_IMAGE is not set")
async def test_plain_image_without_spacesd_is_daemon_agnostic():
    """A VNC-only image (libs/images/plain/ubuntu-xfce-vnc) still creates,
    lists and deletes; only the interfaces report the missing driver."""
    from cua_sandbox import SpacesdNotAvailable

    name = f"cua-e2e-sbx-{uuid.uuid4().hex[:8]}"
    runtime = DockerRuntime(memory_mb=1024)
    runtime.env_ready_timeout = 3
    async with Sandbox.ephemeral(
        Image.from_registry(PLAIN_IMAGE, os_type="linux", kind="container"),
        local=True,
        runtime=runtime,
        name=name,
        telemetry_enabled=False,
    ) as sb:
        assert name in {s.name for s in await Sandbox.list(local=True)}
        with pytest.raises(SpacesdNotAvailable):
            await sb.shell.run("true")
    assert _docker("inspect", "--type", "container", name).returncode != 0, "container leaked"
