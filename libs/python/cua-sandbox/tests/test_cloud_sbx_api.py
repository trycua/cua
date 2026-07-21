from __future__ import annotations

import json
from typing import Any

import httpx
import pytest

from cua_sandbox.image import Image
from cua_sandbox.transport.cloud import (
    CloudTransport,
    cloud_get_vm,
    cloud_list_vms,
    cloud_vm_action,
)

pytestmark = pytest.mark.asyncio


class AsyncClientStub:
    requests: list[httpx.Request] = []
    responses: list[httpx.Response] = []

    def __init__(
        self,
        *,
        base_url: str = "",
        headers: dict[str, str] | None = None,
        timeout: Any = None,
        **_: Any,
    ):
        self.base_url = httpx.URL(base_url)
        self.headers = headers or {}
        self.timeout = timeout

    async def __aenter__(self) -> "AsyncClientStub":
        return self

    async def __aexit__(self, *_: Any) -> None:
        return None

    async def request(
        self,
        method: str,
        path: str,
        *,
        json: Any = None,
        data: Any = None,
        timeout: Any = None,
    ) -> httpx.Response:
        url = httpx.URL(path) if path.startswith("http") else self.base_url.join(path)
        request = httpx.Request(method, url, headers=self.headers, json=json, data=data)
        request.extensions["timeout_arg"] = timeout
        type(self).requests.append(request)
        response = type(self).responses.pop(0)
        response.request = request
        return response

    async def get(self, path: str) -> httpx.Response:
        return await self.request("GET", path)

    async def post(
        self, path: str, *, json: Any = None, data: Any = None, timeout: Any = None
    ) -> httpx.Response:
        return await self.request("POST", path, json=json, data=data, timeout=timeout)

    async def delete(self, path: str) -> httpx.Response:
        return await self.request("DELETE", path)


@pytest.fixture(autouse=True)
def reset_stub(monkeypatch: pytest.MonkeyPatch):
    AsyncClientStub.requests = []
    AsyncClientStub.responses = []
    monkeypatch.setattr(httpx, "AsyncClient", AsyncClientStub)


def response(status: int, payload: Any = None) -> httpx.Response:
    content = b"" if payload is None else json.dumps(payload).encode()
    return httpx.Response(status, content=content, headers={"content-type": "application/json"})


def make_transport(**kwargs: Any) -> CloudTransport:
    transport = object.__new__(CloudTransport)
    CloudTransport.__init__(transport, **kwargs)
    return transport


async def test_create_uses_stable_sbx_aggregate_endpoint():
    AsyncClientStub.responses = [
        response(
            201,
            {
                "name": "demo",
                "status": "running",
                "os": "linux",
                "apiUrl": "/api/svc/cua-sbx-user/demo-abc-api",
            },
        )
    ]
    transport = object.__new__(CloudTransport)
    CloudTransport.__init__(
        transport,
        name="demo",
        api_key="jwt-token",
        base_url="https://run.cua.ai",
        image=Image.linux(),
        cpu=2,
        memory_mb=4096,
        region="us-east-1",
    )
    transport._api_client = AsyncClientStub(
        base_url="https://run.cua.ai",
        headers={"Authorization": "Bearer jwt-token"},
        timeout=30.0,
    )

    created = await transport._create_vm()

    request = AsyncClientStub.requests[0]
    assert request.method == "POST"
    assert request.url == httpx.URL("https://run.cua.ai/api/sbx")
    assert json.loads(request.content) == {
        "name": "demo",
        "os": "linux",
        "cpu": 2,
        "memoryMb": 4096,
    }
    assert request.extensions["timeout_arg"] == 600.0
    assert created["apiUrl"] == "/api/svc/cua-sbx-user/demo-abc-api"
    assert (
        transport._resolve_endpoint(created)
        == "https://run.cua.ai/api/svc/cua-sbx-user/demo-abc-api"
    )


async def test_get_and_list_use_sbx_endpoints():
    AsyncClientStub.responses = [
        response(200, {"name": "demo", "status": "running"}),
        response(200, [{"name": "demo", "status": "running"}]),
    ]

    got = await cloud_get_vm("demo", api_key="jwt-token", base_url="https://run.cua.ai")
    listed = await cloud_list_vms(api_key="jwt-token", base_url="https://run.cua.ai")

    assert got["name"] == "demo"
    assert listed == [{"name": "demo", "status": "running"}]
    assert [request.url.path for request in AsyncClientStub.requests] == [
        "/api/sbx/demo",
        "/api/sbx",
    ]


async def test_delete_uses_ordered_server_side_sbx_delete():
    AsyncClientStub.responses = [response(204)]

    await cloud_vm_action(
        "demo",
        "delete",
        api_key="jwt-token",
        base_url="https://run.cua.ai",
    )

    request = AsyncClientStub.requests[0]
    assert request.method == "DELETE"
    assert request.url.path == "/api/sbx/demo"


async def test_non_delete_vm_actions_remain_on_legacy_endpoint():
    AsyncClientStub.responses = [response(200)]

    await cloud_vm_action(
        "demo",
        "stop",
        api_key="jwt-token",
        base_url="https://api.cua.ai",
    )

    request = AsyncClientStub.requests[0]
    assert request.method == "POST"
    assert request.url.path == "/v1/vms/demo/stop"


async def test_client_credentials_are_exchanged_for_sbx_bearer(monkeypatch: pytest.MonkeyPatch):
    from cua_sandbox import _config
    from cua_sandbox.transport.cloud import _resolve_cloud_token

    monkeypatch.setattr(_config._global_config, "api_key", None)
    monkeypatch.setattr(_config._global_config, "sandbox_client_id", "ukey-demo")
    monkeypatch.setattr(_config._global_config, "sandbox_client_secret", "secret")
    AsyncClientStub.responses = [response(200, {"access_token": "jwt-token"})]

    token = await _resolve_cloud_token(None)

    assert token == "jwt-token"
    request = AsyncClientStub.requests[0]
    assert request.url == httpx.URL(
        "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
    )
    assert request.method == "POST"
    assert request.headers["content-type"].startswith("application/x-www-form-urlencoded")


async def test_non_delete_vm_actions_default_to_legacy_api_host(monkeypatch: pytest.MonkeyPatch):
    from cua_sandbox import _config

    monkeypatch.setattr(_config._global_config, "base_url", "https://api.cua.ai")
    monkeypatch.setattr(_config._global_config, "sandbox_base_url", "https://run.cua.ai")
    AsyncClientStub.responses = [response(200)]

    await cloud_vm_action("demo", "restart", api_key="jwt-token")

    request = AsyncClientStub.requests[0]
    assert request.url == httpx.URL("https://api.cua.ai/v1/vms/demo/restart")


async def test_non_delete_vm_actions_keep_configured_legacy_api_key(
    monkeypatch: pytest.MonkeyPatch,
):
    from cua_sandbox import _config

    monkeypatch.setattr(_config._global_config, "api_key", "sk-legacy")
    monkeypatch.setattr(_config._global_config, "base_url", "https://api.cua.ai")
    AsyncClientStub.responses = [response(200)]

    await cloud_vm_action("demo", "stop")

    request = AsyncClientStub.requests[0]
    assert request.headers["authorization"] == "Bearer sk-legacy"
    assert request.url == httpx.URL("https://api.cua.ai/v1/vms/demo/stop")


async def test_create_rejects_unsupported_fleet_disk_size():
    transport = make_transport(
        name="demo",
        api_key="jwt-token",
        image=Image.linux(),
        disk_gb=128,
    )
    transport._api_client = AsyncClientStub(
        base_url="https://run.cua.ai",
        headers={"Authorization": "Bearer jwt-token"},
        timeout=30.0,
    )

    with pytest.raises(ValueError, match="disk_gb"):
        await transport._create_vm()


async def test_create_rejects_unsupported_fleet_region():
    transport = make_transport(
        name="demo",
        api_key="jwt-token",
        image=Image.linux(),
        region="eu-west-1",
    )
    transport._api_client = AsyncClientStub(
        base_url="https://run.cua.ai",
        headers={"Authorization": "Bearer jwt-token"},
        timeout=30.0,
    )

    with pytest.raises(ValueError, match="region"):
        await transport._create_vm()


async def test_transport_legacy_action_uses_legacy_api_host(monkeypatch: pytest.MonkeyPatch):
    from cua_sandbox import _config

    monkeypatch.setattr(_config._global_config, "base_url", "https://api.cua.ai")
    AsyncClientStub.responses = [response(200)]
    transport = make_transport(name="demo", api_key="sk-legacy", base_url="https://run.cua.ai")

    await transport.suspend_vm()

    request = AsyncClientStub.requests[0]
    assert request.url == httpx.URL("https://api.cua.ai/v1/vms/demo/stop")
    assert request.headers["authorization"] == "Bearer sk-legacy"


async def test_snapshot_fork_keeps_legacy_api_key_path(monkeypatch: pytest.MonkeyPatch):
    from cua_sandbox import _config

    monkeypatch.setattr(_config._global_config, "base_url", "https://api.cua.ai")
    image = Image(
        os_type="linux",
        distro="ubuntu",
        version="24.04",
        kind="vm",
        _snapshot_source={"instance": "source", "snapshot": "snap"},
    )
    transport = make_transport(name="fork", api_key="sk-legacy", image=image)
    AsyncClientStub.responses = [response(201, {"name": "fork", "status": "running"})]

    transport._api_client = AsyncClientStub(
        base_url="https://run.cua.ai",
        headers={"Authorization": "Bearer sk-legacy"},
        timeout=30.0,
    )
    created = await transport._create_vm()

    assert created["name"] == "fork"
    request = AsyncClientStub.requests[0]
    assert request.url == httpx.URL("https://api.cua.ai/v1/vms")
    assert request.headers["authorization"] == "Bearer sk-legacy"


async def test_snapshot_transport_get_and_delete_remain_legacy(
    monkeypatch: pytest.MonkeyPatch,
):
    from cua_sandbox import _config

    monkeypatch.setattr(_config._global_config, "base_url", "https://api.cua.ai")
    image = Image(
        os_type="linux",
        distro="ubuntu",
        version="24.04",
        kind="vm",
        _snapshot_source={"instance": "source", "snapshot": "snap"},
    )
    transport = make_transport(name="fork", api_key="sk-legacy", image=image)
    transport._api_client = AsyncClientStub(
        base_url="https://api.cua.ai",
        headers={"Authorization": "Bearer sk-legacy"},
        timeout=30.0,
    )
    AsyncClientStub.responses = [
        response(200, {"name": "fork", "status": "running"}),
        response(204),
    ]

    await transport._get_vm("fork")
    await transport.delete_vm()

    assert [request.url for request in AsyncClientStub.requests] == [
        httpx.URL("https://api.cua.ai/v1/vms/fork"),
        httpx.URL("https://api.cua.ai/v1/vms/fork"),
    ]
    assert AsyncClientStub.requests[1].method == "DELETE"


async def test_snapshot_fork_honors_explicit_legacy_base_url():
    image = Image(
        os_type="linux",
        distro="ubuntu",
        version="24.04",
        kind="vm",
        _snapshot_source={"instance": "source", "snapshot": "snap"},
    )
    transport = make_transport(
        name="fork",
        api_key="sk-legacy",
        base_url="http://localhost:8082",
        image=image,
    )
    transport._api_client = AsyncClientStub(
        base_url="http://localhost:8082",
        headers={"Authorization": "Bearer sk-legacy"},
        timeout=30.0,
    )
    AsyncClientStub.responses = [response(201, {"name": "fork", "status": "running"})]

    await transport._create_vm()

    assert AsyncClientStub.requests[0].url == httpx.URL("http://localhost:8082/v1/vms")
