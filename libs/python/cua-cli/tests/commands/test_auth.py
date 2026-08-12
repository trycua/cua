"""Tests for authentication commands."""

import argparse
import asyncio
from datetime import UTC, datetime, timedelta
from unittest.mock import patch

from cua_cli.auth.store import OAuthCredentials
from cua_cli.commands import auth


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
        return OAuthCredentials(
            access_token="access-token",
            refresh_token="refresh-token",
            expires_at=datetime.now(UTC) + timedelta(hours=1),
        )

    async def revoke(self, _discovery, _credentials):
        return True


def run(coroutine):
    return asyncio.run(coroutine)


def test_login_uses_device_flow_without_opening_browser(monkeypatch) -> None:
    saved: list[OAuthCredentials] = []
    monkeypatch.setattr(auth, "OidcClient", FakeOidcClient)
    monkeypatch.setattr(auth, "run_async", run)
    monkeypatch.setattr(auth, "save_credentials", saved.append)

    with patch.object(auth.webbrowser, "open") as browser_open:
        result = auth.cmd_login(argparse.Namespace(no_browser=True))

    assert result == 0
    assert saved[0].access_token == "access-token"
    browser_open.assert_not_called()


def test_logout_clears_local_credentials_when_remote_revocation_fails(monkeypatch) -> None:
    credentials = OAuthCredentials(
        access_token="access-token",
        refresh_token="refresh-token",
        expires_at=datetime.now(UTC) + timedelta(hours=1),
    )
    monkeypatch.setattr(auth, "load_credentials", lambda: credentials)
    monkeypatch.setattr(auth, "OidcClient", FakeOidcClient)
    monkeypatch.setattr(auth, "run_async", run)

    with patch.object(auth, "clear_credentials") as clear:
        result = auth.cmd_logout(argparse.Namespace())

    assert result == 0
    clear.assert_called_once()


def test_status_does_not_expose_token(capsys, monkeypatch) -> None:
    credentials = OAuthCredentials(
        access_token="secret-access-token",
        refresh_token="secret-refresh-token",
        expires_at=datetime.now(UTC) + timedelta(hours=1),
    )
    monkeypatch.setattr(auth, "load_credentials", lambda: credentials)

    assert auth.cmd_status(argparse.Namespace()) == 0
    output = capsys.readouterr().out
    assert "secret-access-token" not in output
    assert "secret-refresh-token" not in output
