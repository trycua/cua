"""Tests for authenticated run API requests."""

import pytest
from cua_cli.api.client import CloudAPIClient


@pytest.mark.asyncio
async def test_cloud_client_uses_refreshable_bearer_token(monkeypatch) -> None:
    async def access_token() -> str:
        return "refreshed-access-token"

    monkeypatch.setattr("cua_cli.api.client.get_access_token", access_token)
    client = CloudAPIClient()

    headers = await client._headers()

    assert client.api_base == "https://run.cua.ai"
    assert headers["Authorization"] == "Bearer refreshed-access-token"
