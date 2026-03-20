"""Integration tests — cloud sandbox via CUA API.

    CUA_API_KEY=sk-... pytest tests/test_cloud.py -v -s

Requires a running cloud VM. Set CUA_TEST_CLOUD_VM_NAME to the VM name.
"""

from __future__ import annotations

import os

import pytest
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio

API_KEY = os.environ.get("CUA_API_KEY")
VM_NAME = os.environ.get("CUA_TEST_CLOUD_VM_NAME", "steady-bluebird")

skip_no_key = pytest.mark.skipif(not API_KEY, reason="CUA_API_KEY not set")


@skip_no_key
async def test_cloud_connect_by_name():
    """Connect to an existing cloud VM by name and take a screenshot."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    screenshot = await sb.screenshot()
    assert screenshot[:4] == b"\x89PNG"
    assert len(screenshot) > 1000
    await sb.disconnect()


@skip_no_key
async def test_cloud_shell():
    """Run a shell command on a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    result = await sb.shell.run("echo hello-cloud")
    assert result.success
    assert "hello-cloud" in result.stdout
    await sb.disconnect()


@skip_no_key
async def test_cloud_screen_size():
    """Get screen dimensions from a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    w, h = await sb.get_dimensions()
    assert w > 0
    assert h > 0
    await sb.disconnect()


@skip_no_key
async def test_cloud_keyboard_mouse():
    """Basic keyboard and mouse operations on a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    await sb.mouse.move(100, 100)
    await sb.mouse.click(100, 100)
    await sb.keyboard.type("hello")
    await sb.disconnect()


@skip_no_key
async def test_cloud_environment():
    """Get environment info from a cloud VM."""
    sb = await Sandbox.connect(VM_NAME, api_key=API_KEY)
    env = await sb.get_environment()
    assert env in ("windows", "mac", "linux", "browser")
    await sb.disconnect()


async def test_cloud_no_api_key_errors():
    """Connecting with no API key gives a clear error."""
    old = os.environ.pop("CUA_API_KEY", None)
    try:
        with pytest.raises(ValueError, match="No CUA API key found"):
            await Sandbox.connect("anything")
    finally:
        if old:
            os.environ["CUA_API_KEY"] = old


async def test_cloud_no_image_no_name_errors():
    """Creating without an image raises a clear error."""
    with pytest.raises((ValueError, TypeError)):
        await Sandbox._create(api_key="sk-test-fake-key")


@skip_no_key
async def test_cloud_ephemeral_linux():
    """Create an ephemeral cloud Linux VM, use it, and destroy on exit."""
    async with Sandbox.ephemeral(Image.linux(), api_key=API_KEY) as sb:
        assert sb.name is not None
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000
        result = await sb.shell.run("echo ephemeral-test")
        assert result.success
        assert "ephemeral-test" in result.stdout


@skip_no_key
async def test_cloud_ephemeral_android():
    """Create an ephemeral Android cloud VM, verify screenshot and display URL."""
    async with Sandbox.ephemeral(Image.android("14"), api_key=API_KEY) as sb:
        assert sb.name is not None
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000
        env = await sb.get_environment()
        assert env == "android"
        display_url = await sb.get_display_url(share=True)
        assert ".cua.sh" in display_url
        assert "password=" in display_url


@skip_no_key
async def test_cloud_invalid_api_key_errors():
    """An invalid (reversed) API key should get an HTTP error from the API."""
    reversed_key = API_KEY[::-1]
    with pytest.raises(Exception):
        sb = await Sandbox.connect(VM_NAME, api_key=reversed_key)
        await sb.screenshot()
        await sb.disconnect()
