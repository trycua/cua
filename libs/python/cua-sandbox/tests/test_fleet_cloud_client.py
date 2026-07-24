import cua_sandbox.transport.fleet_cloud as fleet_cloud
import pytest
from cua_sandbox._config import _global_config, configure
from cua_sandbox.transport.fleet_cloud import _FleetClient


@pytest.fixture(autouse=True)
def reset_config():
    _global_config.access_token = None
    _global_config.client_id = None
    _global_config.client_secret = None


def test_static_access_token_constructs_generated_client(monkeypatch):
    configure(
        access_token="temporary-device-token",
        client_id="client-id",
        client_secret="client-secret",
    )
    generated_client = object()
    calls = []

    monkeypatch.setattr(fleet_cloud, "CyclopsHttpClient", lambda: object())
    monkeypatch.setattr(
        fleet_cloud.CyclopsClient,
        "connect_with_access_token",
        classmethod(
            lambda cls, configuration, access_token, http_client: calls.append(
                (configuration, access_token, http_client)
            )
            or generated_client
        ),
    )

    client = _FleetClient()

    assert client._client is generated_client
    assert calls[0][0].base_url == "https://run.cua.ai"
    assert calls[0][1] == "temporary-device-token"


def test_client_credentials_remain_the_fallback(monkeypatch):
    configure(client_id="client-id", client_secret="client-secret")
    generated_client = object()
    calls = []

    monkeypatch.setattr(fleet_cloud, "CyclopsHttpClient", lambda: object())
    monkeypatch.setattr(
        fleet_cloud.CyclopsClient,
        "connect",
        classmethod(
            lambda cls, configuration, http_client: calls.append((configuration, http_client))
            or generated_client
        ),
    )

    client = _FleetClient()

    assert client._client is generated_client
    assert calls[0][0].credentials.client_id == "client-id"


@pytest.mark.asyncio
async def test_list_pools_enumerates_namespaces_with_static_bearer_token():
    client = _FleetClient.__new__(_FleetClient)

    class Http:
        async def execute(self, request):
            assert request.url.endswith("/api/namespaces")
            assert [(header.name, header.value) for header in request.headers] == [
                ("Authorization", "Bearer temporary-device-token")
            ]
            return type(
                "Response", (), {"status": 200, "body": b'{"items":[{"name":"one"},"two"]}'}
            )()

    class SDK:
        async def list_pools(self, namespace):
            return [namespace]

    client._base_url = "https://fleet.example"
    client._access_token = "temporary-device-token"
    client._http_client = Http()
    client._client = SDK()
    assert await client.list_pools() == ["one", "two"]


@pytest.mark.asyncio
async def test_list_pools_401_requires_reauthentication_without_retry():
    client = _FleetClient.__new__(_FleetClient)
    requests = []

    class Http:
        async def execute(self, request):
            requests.append(request)
            return type("Response", (), {"status": 401, "body": b"expired"})()

    client._base_url = "https://fleet.example"
    client._access_token = "temporary-device-token"
    client._http_client = Http()

    with pytest.raises(RuntimeError, match="authenticate again") as error:
        await client.list_pools()

    assert len(requests) == 1
    assert "temporary-device-token" not in str(error.value)


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
