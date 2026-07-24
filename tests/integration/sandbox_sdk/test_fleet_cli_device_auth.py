"""Opt-in Fleet device-auth integration coverage using public SDK APIs only."""

from __future__ import annotations

import os
import uuid

import pytest
from cua_sandbox import Image, Sandbox, configure


def _safe_device_access_token() -> str:
    pytest.skip(
        "A credential-safe public CLI device-token handoff is not available; the live Fleet test remains gated to avoid reading, exporting, or logging credentials."
    )


@pytest.mark.asyncio
async def test_fleet_cli_device_auth_create_use_delete_and_verify_absence():
    image = os.environ.get("CUA_FLEET_TEST_IMAGE")
    if not image:
        pytest.skip("CUA_FLEET_TEST_IMAGE must name an approved Fleet image")
    configure(access_token=_safe_device_access_token())
    name = f"fleet-device-auth-{uuid.uuid4().hex[:12]}"
    try:
        sandbox = await Sandbox.create(Image.from_registry(image), name=name)
        result = await sandbox.shell.run("printf fleet-device-auth-ok")
        assert result.success
        assert result.stdout == "fleet-device-auth-ok"
    finally:
        await Sandbox.delete(name)
        assert name not in {sandbox_info.name for sandbox_info in await Sandbox.list()}
