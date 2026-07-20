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


def test_get_pool_reads_named_resource(monkeypatch):
    client = make_client()
    calls = []

    def get(url, **kwargs):
        calls.append(url)
        return response(200, {"metadata": {"namespace": "demo", "name": "demo"}})

    monkeypatch.setattr(httpx, "get", get)

    client.get_pool("demo")

    assert calls == [
        "https://run.cua.ai/api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo"
    ]


def test_list_pools_enumerates_namespaces(monkeypatch):
    client = make_client()
    calls = []
    namespace_url = "https://run.cua.ai/api/namespaces"
    first_pool_url = (
        "https://run.cua.ai/api/k8s/apis/cua.ai/v1/namespaces/first/osgymworkspacepools"
    )
    second_pool_url = (
        "https://run.cua.ai/api/k8s/apis/cua.ai/v1/namespaces/second/osgymworkspacepools"
    )
    responses = {
        namespace_url: response(
            200, {"items": [{"name": "first"}, {"metadata": {"name": "second"}}]}
        ),
        first_pool_url: response(200, {"items": [{"metadata": {"name": "first"}}]}),
        second_pool_url: response(200, [{"metadata": {"name": "second"}}]),
    }

    def get(url, **kwargs):
        calls.append(url)
        return responses[url]

    monkeypatch.setattr(httpx, "get", get)

    pools = client.list_pools()

    assert calls == [namespace_url, first_pool_url, second_pool_url]
    assert [pool["metadata"]["name"] for pool in pools] == ["first", "second"]


@pytest.mark.parametrize("status_code", [502, 503, 504])
def test_wait_pool_ready_retries_transient_gateway_errors(monkeypatch, status_code):
    client = make_client()
    ready_pool = {
        "metadata": {"namespace": "demo", "name": "demo"},
        "status": {"availableCount": 1},
    }
    responses = iter([response(status_code, {"error": "gateway"}), response(200, ready_pool)])
    sleep = Mock()

    monkeypatch.setattr(httpx, "get", lambda *args, **kwargs: next(responses))
    monkeypatch.setattr("cua_sandbox.transport.fleet_cloud.time.sleep", sleep)

    assert client.wait_pool_ready(ready_pool) == ready_pool
    sleep.assert_called_once_with(2)


def test_wait_pool_ready_timeout_includes_gateway_details(monkeypatch):
    client = make_client()
    pool = {"metadata": {"namespace": "demo", "name": "demo"}}
    gateway_response = httpx.Response(
        502,
        text="upstream unavailable",
        headers={"X-Request-ID": "request-123"},
        request=httpx.Request("GET", "https://run.cua.ai/test"),
    )

    monkeypatch.setattr(httpx, "get", lambda *args, **kwargs: gateway_response)
    monkeypatch.setattr(
        "cua_sandbox.transport.fleet_cloud.time.monotonic", Mock(side_effect=[0, 1])
    )

    with pytest.raises(TimeoutError) as error:
        client.wait_pool_ready(pool, time_to_start=1)

    message = str(error.value)
    assert (
        "endpoint='https://run.cua.ai/api/k8s/apis/cua.ai/v1/namespaces/demo/osgymworkspacepools/demo'"
        in message
    )
    assert "namespace='demo'" in message
    assert "pool='demo'" in message
    assert "status_code=502" in message
    assert "response_body='upstream unavailable'" in message
    assert "x-request-id" in message


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
