"""Opt-in Fleet device-auth integration coverage.

A credential-safe public CLI-to-SDK token handoff is not available on current
main. Keep this test as an explicit gate until that public surface exists;
never add a token environment variable, keyring read, or token-printing helper.
"""

import os

import pytest


@pytest.mark.asyncio
async def test_fleet_cli_device_auth_requires_safe_public_handoff():
    if not os.environ.get("CUA_FLEET_TEST_IMAGE"):
        pytest.skip("CUA_FLEET_TEST_IMAGE must name an approved Fleet image")
    pytest.skip(
        "A credential-safe public CLI device-token handoff is not available; "
        "the live Fleet test remains gated to avoid reading, exporting, or logging credentials."
    )
