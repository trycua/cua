import logging
from types import SimpleNamespace

import pytest
from cua_sandbox.transport.fleet_cloud import _FleetClient, _GitHubActionsAccessTokenProvider
from fleet_sdk import (
    AccessTokenProviderError,
    CyclopsClient,
    CyclopsTokenProviderConfiguration,
    HttpClient,
    HttpResponse,
)


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
async def test_list_pools_does_not_discover_namespaces():
    client = _FleetClient.__new__(_FleetClient)

    class Http:
        async def execute(self, request):
            pytest.fail("Fleet listing must not discover namespaces")

    class SDK:
        async def list_pools(self, namespace):
            pytest.fail("Fleet listing requires an explicit namespace")

    client._base_url = "https://fleet.example"
    client._http_client = Http()
    client._client = SDK()

    with pytest.raises(NotImplementedError, match="exact sandbox name"):
        await client.list_pools()


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
async def test_github_actions_token_provider_refreshes_and_caches_token():
    calls = []

    async def request(url, headers):
        calls.append((url, headers))
        return 200, {"value": "refreshed-token"}

    provider = _GitHubActionsAccessTokenProvider(
        "initial-token",
        environ={
            "ACTIONS_ID_TOKEN_REQUEST_URL": "https://github.example/oidc?existing=value",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-token",
        },
        request=request,
    )

    assert await provider.get_access_token(False) == "initial-token"
    assert calls == []
    assert await provider.get_access_token(True) == "refreshed-token"
    assert await provider.get_access_token(False) == "refreshed-token"
    assert calls == [
        (
            "https://github.example/oidc?existing=value&audience=fleets",
            {
                "Accept": "application/json",
                "Authorization": "bearer request-token",
            },
        )
    ]


@pytest.mark.asyncio
async def test_github_actions_token_provider_maps_refresh_failures():
    async def request(url, headers):
        return 403, {"message": "forbidden"}

    provider = _GitHubActionsAccessTokenProvider(
        "initial-token",
        environ={
            "ACTIONS_ID_TOKEN_REQUEST_URL": "https://github.example/oidc",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-token",
        },
        request=request,
    )

    with pytest.raises(AccessTokenProviderError.Failed, match="HTTP 403"):
        await provider.get_access_token(True)


@pytest.mark.asyncio
async def test_generated_sdk_retries_unauthorized_request_with_refreshed_github_token():
    refresh_calls = 0

    async def refresh_token(url, headers):
        nonlocal refresh_calls
        refresh_calls += 1
        return 200, {"value": "refreshed-token"}

    class ScriptedHttpClient(HttpClient):
        def __init__(self):
            self.authorization = []

        async def execute(self, request):
            authorization = next(
                header.value for header in request.headers if header.name.lower() == "authorization"
            )
            self.authorization.append(authorization)
            if len(self.authorization) == 1:
                return HttpResponse(status=401, headers=[], body=b'{"error":"expired"}')
            return HttpResponse(status=200, headers=[], body=b'{"items":[]}')

    provider = _GitHubActionsAccessTokenProvider(
        "initial-token",
        environ={
            "ACTIONS_ID_TOKEN_REQUEST_URL": "https://github.example/oidc",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-token",
        },
        request=refresh_token,
    )
    http_client = ScriptedHttpClient()
    client = CyclopsClient.connect_with_access_token_provider(
        CyclopsTokenProviderConfiguration(
            base_url="https://fleet.example",
            pool_poll_interval_ms=1,
            pool_poll_limit=1,
            claim_poll_interval_ms=1,
            claim_poll_limit=1,
        ),
        provider,
        http_client,
    )

    assert await client.list_pools("cua-cli-wif-smoke") == []
    assert http_client.authorization == ["Bearer initial-token", "Bearer refreshed-token"]
    assert refresh_calls == 1


@pytest.mark.asyncio
async def test_fleet_client_uses_static_workload_token_and_closes_http_client(monkeypatch, caplog):
    monkeypatch.delenv("ACTIONS_ID_TOKEN_REQUEST_URL", raising=False)
    monkeypatch.delenv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", raising=False)
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


def test_fleet_client_uses_refreshable_provider_in_github_actions(monkeypatch):
    from cua_sandbox.transport import fleet_cloud

    calls = {}

    class HttpClient:
        async def aclose(self):
            pass

    class Client:
        @staticmethod
        def connect_with_access_token_provider(configuration, provider, http_client):
            calls["provider"] = provider
            return "token-client"

    monkeypatch.setenv("ACTIONS_ID_TOKEN_REQUEST_URL", "https://github.example/oidc")
    monkeypatch.setenv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token")
    monkeypatch.setattr(fleet_cloud, "get_fleet_token", lambda: "initial-token")
    monkeypatch.setattr(fleet_cloud, "get_fleet_base_url", lambda: "https://fleet.example/")
    monkeypatch.setattr(fleet_cloud, "CyclopsHttpClient", HttpClient)
    monkeypatch.setattr(fleet_cloud, "CyclopsClient", Client)

    client = _FleetClient()

    assert client._client == "token-client"
    assert isinstance(calls["provider"], _GitHubActionsAccessTokenProvider)


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
