import pytest
from cua_sandbox.transport.fleet_cloud import _FleetClient


@pytest.mark.asyncio
async def test_list_pools_enumerates_namespaces(monkeypatch):
    client = _FleetClient.__new__(_FleetClient)

    class Http:
        async def execute(self, request):
            assert request.url.endswith("/api/namespaces")
            return type("Response", (), {"status": 200, "body": b'{"items":[{"name":"one"},"two"]}'})()

    class SDK:
        async def list_pools(self, namespace):
            return [namespace]

    client._base_url = "https://fleet.example"
    client._http_client = Http()
    client._client = SDK()
    assert await client.list_pools() == ["one", "two"]


@pytest.mark.asyncio
async def test_service_request_delegates_to_generated_client():
    client = _FleetClient.__new__(_FleetClient)
    calls = []

    class SDK:
        async def service_request(self, *args):
            calls.append(args)
            return "response"

    client._client = SDK()
    assert await client.service_request("sandbox", "server", "/status", "request") == "response"
    assert calls == [("sandbox", "server", "/status", "request")]


@pytest.mark.asyncio
async def test_pool_lookup_matches_sdk_resources():
    client = _FleetClient.__new__(_FleetClient)
    pool = type("Pool", (), {"metadata": type("Metadata", (), {"name": "demo"})()})()

    async def list_pools():
        return [pool]

    client.list_pools = list_pools
    assert await client.get_pool("demo") is pool
    with pytest.raises(LookupError):
        await client.get_pool("missing")
