from __future__ import annotations

import json
import re
from types import SimpleNamespace

import pytest
from fleet_sdk import SdkError

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_namespace_name,
    cleanup_namespace,
    collect_resource_inventory,
    is_not_found_error,
    wait_namespace_absent,
    write_summary,
)


def test_build_namespace_name_is_dns_safe_and_bounded() -> None:
    name = build_namespace_name("published-package", "1234567890", "2")
    assert name == "cua-live-published-package-1234567890-2"
    assert len(name) <= 63


def test_build_namespace_name_normalizes_invalid_overlong_input() -> None:
    name = build_namespace_name("Published_Package!!!" + "X" * 100, "RUN__ID", "2!!")

    assert len(name) <= 63
    assert re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", name)
    assert name.startswith("cua-live-published-package-")


def test_assert_template_contract_accepts_server_port_8000() -> None:
    probes = SimpleNamespace(
        to_json=lambda: json.dumps(
            {"readinessProbe": {"tcpSocket": {"port": 8000}}}
        )
    )
    template = SimpleNamespace(
        spec=SimpleNamespace(
            vm_template=SimpleNamespace(
                services=[SimpleNamespace(name="server", target_port=8000)],
                probes=probes,
            )
        )
    )
    assert_template_contract(template, expected_port=8000)


def test_assert_template_contract_rejects_wrong_service_port() -> None:
    probes = SimpleNamespace(
        to_json=lambda: json.dumps(
            {"readinessProbe": {"tcpSocket": {"port": 8000}}}
        )
    )
    template = SimpleNamespace(
        spec=SimpleNamespace(
            vm_template=SimpleNamespace(
                services=[SimpleNamespace(name="server", target_port=5000)],
                probes=probes,
            )
        )
    )
    with pytest.raises(AssertionError, match="target_port"):
        assert_template_contract(template, expected_port=8000)


def test_assert_template_contract_rejects_missing_server_service() -> None:
    probes = SimpleNamespace(
        to_json=lambda: json.dumps(
            {"readinessProbe": {"tcpSocket": {"port": 8000}}}
        )
    )
    template = SimpleNamespace(
        spec=SimpleNamespace(
            vm_template=SimpleNamespace(
                services=[SimpleNamespace(name="other", target_port=8000)],
                probes=probes,
            )
        )
    )

    with pytest.raises(AssertionError, match="server service"):
        assert_template_contract(template, expected_port=8000)


def test_is_not_found_error_accepts_only_public_sdk_404_status() -> None:
    assert is_not_found_error(SdkError.Status("get namespace", 404, b"not found"))
    assert not is_not_found_error(SdkError.Status("get namespace", 500, b"failure"))


@pytest.mark.asyncio
async def test_collect_resource_inventory_lists_owned_resources() -> None:
    def resource(name: str):
        return SimpleNamespace(metadata=SimpleNamespace(name=name))

    class FakeClient:
        async def get_namespace(self, name: str):
            return SimpleNamespace(name=name)

        async def list_templates(self, name: str):
            return [resource("template-a")]

        async def list_pools(self, name: str):
            return [resource("pool-a")]

        async def list_claims(self, name: str):
            return [resource("claim-a")]

    assert await collect_resource_inventory(FakeClient(), "demo") == {
        "templates": ["template-a"],
        "pools": ["pool-a"],
        "claims": ["claim-a"],
    }


@pytest.mark.asyncio
async def test_wait_namespace_absent_polls_until_public_sdk_404() -> None:
    calls = 0

    class FakeClient:
        async def get_namespace(self, name: str):
            nonlocal calls
            calls += 1
            if calls == 1:
                return SimpleNamespace(name=name)
            raise SdkError.Status("get namespace", 404, b"not found")

    assert await wait_namespace_absent(FakeClient(), "demo", timeout=1, interval=0)
    assert calls == 2


def test_write_summary_recursively_redacts_sensitive_values(tmp_path) -> None:
    path = tmp_path / "summary.json"
    write_summary(
        path,
        {
            "authorization": "Bearer top-secret",
            "token": "token-value",
            "client_secret": "client-secret-value",
            "nested": {
                "Authorization": "Nested Bearer secret",
                "items": [{"TOKEN": "nested-token"}, {"safe": "visible"}],
            },
        },
    )

    rendered = path.read_text()
    summary = json.loads(rendered)

    for secret in (
        "Bearer top-secret",
        "token-value",
        "client-secret-value",
        "Nested Bearer secret",
        "nested-token",
    ):
        assert secret not in rendered
    assert summary["authorization"] == "<redacted>"
    assert summary["token"] == "<redacted>"
    assert summary["client_secret"] == "<redacted>"
    assert summary["nested"]["Authorization"] == "<redacted>"
    assert summary["nested"]["items"][0]["TOKEN"] == "<redacted>"
    assert summary["nested"]["items"][1]["safe"] == "visible"


@pytest.mark.asyncio
async def test_cleanup_namespace_treats_delete_404_as_success(monkeypatch) -> None:
    class FakeClient:
        async def get_namespace(self, name: str):
            return SimpleNamespace(name=name)

        async def delete_namespace(self, name: str):
            raise SdkError.Status("delete namespace", 404, b"not found")

    class FakeHttpClient:
        closed = False

        async def aclose(self):
            self.closed = True

    client = FakeClient()
    http_client = FakeHttpClient()
    monkeypatch.setattr(
        "tests.live.fleet_e2e_support.build_fleet_client",
        lambda: (client, http_client),
    )

    assert await cleanup_namespace("demo")
    assert http_client.closed


@pytest.mark.asyncio
async def test_cleanup_namespace_propagates_non_404_delete_error(monkeypatch) -> None:
    class FakeClient:
        async def get_namespace(self, name: str):
            return SimpleNamespace(name=name)

        async def delete_namespace(self, name: str):
            raise SdkError.Status("delete namespace", 500, b"failure")

    class FakeHttpClient:
        closed = False

        async def aclose(self):
            self.closed = True

    client = FakeClient()
    http_client = FakeHttpClient()
    monkeypatch.setattr(
        "tests.live.fleet_e2e_support.build_fleet_client",
        lambda: (client, http_client),
    )

    with pytest.raises(SdkError.Status) as error:
        await cleanup_namespace("demo")
    assert error.value.status == 500
    assert http_client.closed
