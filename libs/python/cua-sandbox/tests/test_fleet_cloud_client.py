from unittest.mock import Mock

import httpx
import pytest
from cua_sandbox.transport.fleet_cloud import _FleetClient


def make_client():
    client = _FleetClient.__new__(_FleetClient)
    client._base_url = "https://run.cua.ai"
    client._headers = {"Authorization": "Bearer token"}
    return client


def response(status_code, payload=None):
    request = httpx.Request("POST", "https://run.cua.ai/test")
    return httpx.Response(status_code, json=payload or {}, request=request)


def test_create_pool_creates_namespace_before_pool(monkeypatch):
    client = make_client()
    calls = []

    def post(url, **kwargs):
        calls.append((url, kwargs))
        return response(201, {"metadata": {"namespace": "demo", "name": "demo"}})

    monkeypatch.setattr(httpx, "post", post)

    client.create_pool({"namespace": "demo", "spec": {"replicas": 1}})

    assert calls[0][0] == "https://run.cua.ai/api/namespaces"
    assert calls[0][1]["json"] == {"name": "demo"}
    assert calls[1][0].endswith("/namespaces/demo/osgymworkspacepools")


def test_create_pool_rolls_back_new_namespace_when_pool_creation_fails(monkeypatch):
    client = make_client()
    deleted = []
    posts = iter([response(201), response(403)])

    monkeypatch.setattr(httpx, "post", lambda *args, **kwargs: next(posts))
    monkeypatch.setattr(httpx, "delete", lambda url, **kwargs: deleted.append(url) or response(204))

    with pytest.raises(httpx.HTTPStatusError):
        client.create_pool({"namespace": "demo", "spec": {"replicas": 1}})

    assert deleted == ["https://run.cua.ai/api/namespaces/demo"]


def test_delete_pool_deletes_pool_then_namespace(monkeypatch):
    client = make_client()
    deleted = []
    monkeypatch.setattr(httpx, "delete", lambda url, **kwargs: deleted.append(url) or response(204))

    client.delete_pool({"metadata": {"namespace": "demo", "name": "demo"}})

    assert deleted == [
        "https://run.cua.ai/api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo",
        "https://run.cua.ai/api/namespaces/demo",
    ]


def test_create_claim_waits_for_available_pool_before_post(monkeypatch):
    client = make_client()
    pool = {"metadata": {"namespace": "demo", "name": "demo"}}
    ready_pool = {
        "metadata": {"namespace": "demo", "name": "demo"},
        "status": {"availableCount": 1},
    }
    monkeypatch.setattr(client, "wait_pool_ready", Mock(return_value=ready_pool))
    monkeypatch.setattr(
        httpx,
        "post",
        Mock(return_value=response(201, {"metadata": {"namespace": "demo", "name": "claim"}})),
    )

    client.create_claim({"pool": pool})

    client.wait_pool_ready.assert_called_once_with(pool)
    assert httpx.post.call_args.args[0].endswith("/namespaces/demo/osgymsandboxclaims")


def test_wait_claim_reports_server_service():
    client = make_client()
    claim = {"metadata": {"namespace": "demo", "name": "claim"}}
    statuses = iter(
        [
            response(200, {"status": {"phase": "Bound", "sandbox": {"name": "sandbox"}}}),
        ]
    )

    import cua_sandbox.transport.fleet_cloud as module

    original_get = module.httpx.get
    module.httpx.get = lambda *args, **kwargs: next(statuses)
    try:
        assert client.wait_claim(claim)["services"] == ["server"]
    finally:
        module.httpx.get = original_get
