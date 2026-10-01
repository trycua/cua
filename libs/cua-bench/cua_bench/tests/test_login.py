"""Tests for the CUA Bench device authorization command."""

import argparse
import asyncio
from datetime import UTC, datetime, timedelta

from cua_bench.cli.commands import login


class FakeOidcClient:
    async def discover(self):
        return object()

    async def request_device_code(self, _discovery):
        return type(
            "DeviceCode",
            (),
            {
                "verification_uri": "https://run.cua.ai/device",
                "verification_uri_complete": "https://run.cua.ai/device?user_code=ABCD-EFGH",
                "user_code": "ABCD-EFGH",
            },
        )()

    async def poll_for_tokens(self, _discovery, _device_code):
        return type(
            "Credentials",
            (),
            {
                "access_token": "access-token",
                "expires_at": datetime.now(UTC) + timedelta(hours=1),
            },
        )()


def run(coroutine):
    return asyncio.run(coroutine)


def test_login_uses_device_authorization_and_saves_access_token(monkeypatch) -> None:
    saved_tokens: list[str] = []
    monkeypatch.setattr(login, "OidcClient", FakeOidcClient)
    monkeypatch.setattr(login, "run_async", run)
    monkeypatch.setattr(login, "save_token", saved_tokens.append)

    result = login.execute(argparse.Namespace(force=True, no_browser=True))

    assert result == 0
    assert saved_tokens == ["access-token"]
