import logging
from types import SimpleNamespace

import pytest
from cua_sandbox.transport.fleet_cloud import _FleetClient


@pytest.mark.asyncio
async def test_wait_pool_returns_once_a_replica_is_ready():
    client = _FleetClient.__new__(_FleetClient)
    warming = SimpleNamespace(
        metadata=SimpleNamespace(name="demo", namespace="demo"),
        status=SimpleNamespace(replicas=1, ready_replicas=0, selector=None),
    )
    ready = SimpleNamespace(
        metadata=SimpleNamespace(name="demo", namespace="demo"),
        status=SimpleNamespace(replicas=1, ready_replicas=1, selector=None),
    )
    listings = iter([[warming], [ready]])

    class SDK:
        async def list_pools(self, namespace):
            assert namespace == "demo"
            return next(listings)

    client._client = SDK()
    assert await client.wait_pool(warming, poll_interval=0) is ready


@pytest.mark.asyncio
async def test_template_calls_delegate_to_generated_client():
    client = _FleetClient.__new__(_FleetClient)
    calls = []

    class SDK:
        async def reconcile_template(self, request):
            calls.append(("reconcile", request))
            return "template"

        async def create_template(self, request):
            calls.append(("create", request))
            return "template"

        async def get_template(self, namespace, name):
            calls.append(("get", namespace, name))
            return "template"

        async def delete_template(self, template):
            calls.append(("delete", template))

    client._client = SDK()
    assert await client.reconcile_template("request") == "template"
    assert await client.create_template("request") == "template"
    assert await client.get_template("demo", "workspace") == "template"
    assert await client.delete_template("template") is None
    assert calls == [
        ("reconcile", "request"),
        ("create", "request"),
        ("get", "demo", "workspace"),
        ("delete", "template"),
    ]


@pytest.mark.asyncio
async def test_namespace_calls_delegate_to_generated_client():
    client = _FleetClient.__new__(_FleetClient)
    calls = []

    class SDK:
        async def get_namespace(self, name):
            calls.append(("get", name))
            return "namespace"

        async def create_namespace(self, name):
            calls.append(("create", name))
            return "namespace"

        async def delete_namespace(self, name):
            calls.append(("delete", name))

    client._client = SDK()
    assert await client.get_namespace("demo") == "namespace"
    assert await client.create_namespace("demo") == "namespace"
    assert await client.delete_namespace("demo") is None
    assert calls == [("get", "demo"), ("create", "demo"), ("delete", "demo")]


@pytest.mark.asyncio
async def test_list_pools_enumerates_namespaces(monkeypatch):
    client = _FleetClient.__new__(_FleetClient)

    class Http:
        async def execute(self, request):
            assert request.url.endswith("/api/namespaces")
            return type(
                "Response", (), {"status": 200, "body": b'{"items":[{"name":"one"},"two"]}'}
            )()

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
async def test_fleet_client_uses_static_workload_token_and_closes_http_client(monkeypatch, caplog):
    from cua_sandbox.transport import fleet_cloud
    from fleet_sdk import AccessTokenProvider, CyclopsTokenProviderConfiguration

    token = "fleet-workload-token"
    calls = {}
    caplog.set_level(logging.DEBUG)

    class HttpClient:
        close_calls = 0

        async def aclose(self):
            self.close_calls += 1

    class Client:
        @staticmethod
        def connect_with_access_token_provider(configuration, provider, http_client):
            calls.update(
                configuration=configuration,
                provider=provider,
                http_client=http_client,
            )
            return "token-client"

        @staticmethod
        def connect(*args):
            pytest.fail("client-credential connection must not be used with a Fleet token")

    monkeypatch.setattr(fleet_cloud, "get_fleet_token", lambda: token)
    monkeypatch.setattr(fleet_cloud, "get_fleet_base_url", lambda: "https://fleet.example/")
    monkeypatch.setattr(fleet_cloud, "CyclopsHttpClient", HttpClient)
    monkeypatch.setattr(fleet_cloud, "CyclopsClient", Client)

    client = _FleetClient()

    assert client._client == "token-client"
    assert isinstance(calls["configuration"], CyclopsTokenProviderConfiguration)
    assert vars(calls["configuration"]) == {
        "base_url": "https://fleet.example",
        "pool_poll_interval_ms": 2000,
        "pool_poll_limit": 300,
        "claim_poll_interval_ms": 2000,
        "claim_poll_limit": 300,
    }
    assert isinstance(calls["provider"], AccessTokenProvider)
    assert await calls["provider"].get_access_token(False) == token
    assert await calls["provider"].get_access_token(True) == token
    assert token not in caplog.text

    await client.close()
    assert calls["http_client"].close_calls == 1


def test_fleet_client_falls_back_to_client_credentials_without_workload_token(monkeypatch):
    from cua_sandbox.transport import fleet_cloud
    from fleet_sdk import CyclopsConfiguration

    calls = {}

    class HttpClient:
        async def aclose(self):
            pass

    class Client:
        @staticmethod
        def connect(configuration, http_client):
            calls.update(configuration=configuration, http_client=http_client)
            return "credential-client"

        @staticmethod
        def connect_with_access_token_provider(*args):
            pytest.fail("token-provider connection requires a Fleet token")

    monkeypatch.setattr(fleet_cloud, "get_fleet_token", lambda: None)
    monkeypatch.setattr(fleet_cloud, "get_client_id", lambda: "client-id")
    monkeypatch.setattr(fleet_cloud, "get_client_secret", lambda: "client-secret")
    monkeypatch.setattr(fleet_cloud, "get_fleet_base_url", lambda: "https://fleet.example/")
    monkeypatch.setattr(fleet_cloud, "get_token_url", lambda: "https://auth.example/token")
    monkeypatch.setattr(fleet_cloud, "CyclopsHttpClient", HttpClient)
    monkeypatch.setattr(fleet_cloud, "CyclopsClient", Client)

    client = _FleetClient()

    assert client._client == "credential-client"
    assert isinstance(calls["configuration"], CyclopsConfiguration)
    assert vars(calls["configuration"]) == {
        "base_url": "https://fleet.example",
        "token_url": "https://auth.example/token",
        "credentials": calls["configuration"].credentials,
        "pool_poll_interval_ms": 2000,
        "pool_poll_limit": 300,
        "claim_poll_interval_ms": 2000,
        "claim_poll_limit": 300,
    }


def test_fleet_client_requires_credentials_when_no_workload_token_exists(monkeypatch):
    from cua_sandbox.transport import fleet_cloud

    monkeypatch.setattr(fleet_cloud, "get_fleet_token", lambda: None)
    monkeypatch.setattr(fleet_cloud, "get_client_id", lambda: None)
    monkeypatch.setattr(fleet_cloud, "get_client_secret", lambda: None)
    monkeypatch.setattr(
        fleet_cloud,
        "CyclopsHttpClient",
        lambda: pytest.fail("missing credentials must fail before creating an HTTP client"),
    )

    with pytest.raises(
        ValueError,
        match="Fleet cloud sandboxes require CUA_CLIENT_ID and CUA_CLIENT_SECRET",
    ):
        _FleetClient()
