"""Tests for the Keycloak OIDC device flow used by run.cua.ai."""

from collections.abc import Mapping
from datetime import UTC, datetime

import pytest
from cua_cli.auth.oidc import DEVICE_GRANT_TYPE, OidcClient
from cua_cli.auth.store import OAuthCredentials


class ScriptedRequester:
    def __init__(self, responses: list[tuple[int, dict]]) -> None:
        self.responses = responses
        self.calls: list[tuple[str, str, Mapping[str, str] | None]] = []

    async def __call__(
        self, method: str, url: str, form: Mapping[str, str] | None
    ) -> tuple[int, dict]:
        self.calls.append((method, url, form))
        return self.responses.pop(0)


@pytest.mark.asyncio
async def test_device_flow_discovers_requests_and_polls() -> None:
    requester = ScriptedRequester(
        [
            (
                200,
                {
                    "token_endpoint": "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
                    "device_authorization_endpoint": "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/device/code",
                    "revocation_endpoint": "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/revoke",
                },
            ),
            (
                200,
                {
                    "device_code": "device-code",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": "https://run.cua.ai/device",
                    "verification_uri_complete": "https://run.cua.ai/device?user_code=ABCD-EFGH",
                    "expires_in": 600,
                    "interval": 2,
                },
            ),
            (400, {"error": "authorization_pending"}),
            (
                200,
                {
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                },
            ),
        ]
    )
    delays: list[float] = []

    async def sleep(delay: float) -> None:
        delays.append(delay)

    client = OidcClient(request=requester, sleep=sleep)
    discovery = await client.discover()
    device_code = await client.request_device_code(discovery)
    credentials = await client.poll_for_tokens(discovery, device_code)

    assert credentials.access_token == "access-token"
    assert credentials.refresh_token == "refresh-token"
    assert delays == [2]
    assert requester.calls[0] == (
        "GET",
        "https://auth.cua.ai/realms/cyclops-cs/.well-known/openid-configuration",
        None,
    )
    assert requester.calls[1][2] == {
        "client_id": "cua-cli",
        "scope": "openid profile offline_access",
    }
    assert requester.calls[2][2] == {
        "grant_type": DEVICE_GRANT_TYPE,
        "device_code": "device-code",
        "client_id": "cua-cli",
    }


@pytest.mark.asyncio
async def test_refresh_preserves_existing_refresh_token_when_not_rotated() -> None:
    requester = ScriptedRequester(
        [
            (
                200,
                {
                    "access_token": "new-access-token",
                    "expires_in": 3600,
                },
            ),
        ]
    )
    credentials = OAuthCredentials(
        access_token="old-access-token",
        refresh_token="existing-refresh-token",
        expires_at=datetime.now(UTC),
    )
    discovery = type(
        "Discovery",
        (),
        {"token_endpoint": "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"},
    )()

    refreshed = await OidcClient(request=requester).refresh(discovery, credentials)

    assert refreshed.access_token == "new-access-token"
    assert refreshed.refresh_token == "existing-refresh-token"
    assert requester.calls[0][2] == {
        "grant_type": "refresh_token",
        "refresh_token": "existing-refresh-token",
        "client_id": "cua-cli",
    }


@pytest.mark.asyncio
async def test_poll_slow_down_increases_interval() -> None:
    requester = ScriptedRequester(
        [
            (400, {"error": "slow_down"}),
            (200, {"access_token": "access-token", "expires_in": 3600}),
        ]
    )
    delays: list[float] = []

    async def sleep(delay: float) -> None:
        delays.append(delay)

    discovery = type(
        "Discovery",
        (),
        {"token_endpoint": "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"},
    )()
    device_code = type("Device", (), {"device_code": "device", "expires_in": 600, "interval": 3})()

    await OidcClient(request=requester, sleep=sleep).poll_for_tokens(discovery, device_code)

    assert delays == [8]
