from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_namespace_name,
    collect_resource_inventory,
    wait_namespace_absent,
)


def test_build_namespace_name_is_dns_safe_and_bounded() -> None:
    name = build_namespace_name("published-package", "1234567890", "2")
    assert name == "cua-live-published-package-1234567890-2"
    assert len(name) <= 63


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
async def test_wait_namespace_absent_polls_until_404(monkeypatch) -> None:
    class StatusError(Exception):
        def __init__(self, status: int) -> None:
            self.status = status

    calls = 0

    class FakeClient:
        async def get_namespace(self, name: str):
            nonlocal calls
            calls += 1
            if calls == 1:
                return SimpleNamespace(name=name)
            raise StatusError(404)

    monkeypatch.setattr(
        "tests.live.fleet_e2e_support.is_not_found_error",
        lambda error: getattr(error, "status", None) == 404,
    )
    monkeypatch.setattr("asyncio.sleep", lambda _: _completed())

    assert await wait_namespace_absent(FakeClient(), "demo", timeout=1, interval=0)
    assert calls == 2


async def _completed() -> None:
    return None
