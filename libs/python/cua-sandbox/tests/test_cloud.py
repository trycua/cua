"""Integration tests — cloud sandbox via CUA API.

    CUA_API_KEY=sk-... pytest tests/test_cloud.py -v -s

Requires a running cloud VM. Set CUA_TEST_CLOUD_VM_NAME to the VM name.
"""

from __future__ import annotations

import os

import pytest
from cua_sandbox import sandbox

pytestmark = pytest.mark.asyncio

API_KEY = os.environ.get("CUA_API_KEY")
VM_NAME = os.environ.get("CUA_TEST_CLOUD_VM_NAME", "steady-bluebird")

skip_no_key = pytest.mark.skipif(not API_KEY, reason="CUA_API_KEY not set")


@skip_no_key
async def test_cloud_connect_by_name():
    """Connect to an existing cloud VM by name and take a screenshot."""
    async with sandbox(name=VM_NAME, api_key=API_KEY) as sb:
        screenshot = await sb.screenshot()
        assert screenshot[:4] == b"\x89PNG"
        assert len(screenshot) > 1000


@skip_no_key
async def test_cloud_shell():
    """Run a shell command on a cloud VM."""
    async with sandbox(name=VM_NAME, api_key=API_KEY) as sb:
        result = await sb.shell.run("echo hello-cloud")
        assert result.success
        assert "hello-cloud" in result.stdout


@skip_no_key
async def test_cloud_screen_size():
    """Get screen dimensions from a cloud VM."""
    async with sandbox(name=VM_NAME, api_key=API_KEY) as sb:
        w, h = await sb.get_dimensions()
        assert w > 0
        assert h > 0


@skip_no_key
async def test_cloud_keyboard_mouse():
    """Basic keyboard and mouse operations on a cloud VM."""
    async with sandbox(name=VM_NAME, api_key=API_KEY) as sb:
        await sb.mouse.move(100, 100)
        await sb.mouse.click(100, 100)
        await sb.keyboard.type("hello")


@skip_no_key
async def test_cloud_environment():
    """Get environment info from a cloud VM."""
    async with sandbox(name=VM_NAME, api_key=API_KEY) as sb:
        env = await sb.get_environment()
        assert env in ("windows", "mac", "linux", "browser")


async def test_cloud_no_api_key_errors():
    """Calling sandbox() with no API key gives a clear error."""
    # Temporarily unset env var if present
    old = os.environ.pop("CUA_API_KEY", None)
    try:
        with pytest.raises(ValueError, match="No CUA API key found"):
            async with sandbox(name="anything") as _sb:
                pass
    finally:
        if old:
            os.environ["CUA_API_KEY"] = old


async def test_cloud_no_image_no_name_errors():
    """Calling sandbox() with API key but no name/image gives a clear error."""
    with pytest.raises(ValueError, match="Cannot create a cloud VM without an image"):
        async with sandbox(api_key="sk-test-fake-key") as _sb:
            pass


@skip_no_key
async def test_cloud_invalid_api_key_errors():
    """An invalid (reversed) API key should get an HTTP error from the API."""
    reversed_key = API_KEY[::-1]
    with pytest.raises(Exception):
        async with sandbox(name=VM_NAME, api_key=reversed_key) as sb:
            await sb.screenshot()
