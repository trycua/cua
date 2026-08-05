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
