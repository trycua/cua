from __future__ import annotations

import json
import re
from types import SimpleNamespace

import pytest
from fleet_sdk import SdkError

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_namespace_name,
    collect_resource_inventory,
    is_not_found_error,
    wait_namespace_absent,
    write_summary,
)


def test_live_test_requires_both_oauth_values(monkeypatch) -> None:
    monkeypatch.delenv("CUA_CLIENT_ID", raising=False)
    monkeypatch.delenv("CUA_CLIENT_SECRET", raising=False)
    from tests.live.test_fleet_ephemeral import has_oauth_credentials

    assert not has_oauth_credentials()
    monkeypatch.setenv("CUA_CLIENT_ID", "client")
    assert not has_oauth_credentials()
    monkeypatch.setenv("CUA_CLIENT_SECRET", "secret")
    assert has_oauth_credentials()


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
            "api_key": "api-key-value",
            "clientSecret": "camel-client-secret-value",
            "accessToken": "access-token-value",
            "refreshToken": "refresh-token-value",
            "nested": {
                "Authorization": "Nested Bearer secret",
                "x-api-key": "header-api-key-value",
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
        "api-key-value",
        "camel-client-secret-value",
        "access-token-value",
        "refresh-token-value",
        "header-api-key-value",
        "Nested Bearer secret",
        "nested-token",
    ):
        assert secret not in rendered
    assert summary["authorization"] == "<redacted>"
    assert summary["token"] == "<redacted>"
    assert summary["client_secret"] == "<redacted>"
    assert summary["api_key"] == "<redacted>"
    assert summary["clientSecret"] == "<redacted>"
    assert summary["accessToken"] == "<redacted>"
    assert summary["refreshToken"] == "<redacted>"
    assert summary["nested"]["Authorization"] == "<redacted>"
    assert summary["nested"]["x-api-key"] == "<redacted>"
    assert summary["nested"]["items"][0]["TOKEN"] == "<redacted>"
    assert summary["nested"]["items"][1]["safe"] == "visible"


def test_live_cleanup_exposes_no_explicit_namespace_deletion_api() -> None:
    from cua_sandbox import Sandbox as CuaSandbox
    from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
    from tests.live import fleet_e2e_support, test_fleet_ephemeral as live_test

    assert not hasattr(fleet_e2e_support, "cleanup_namespace")
    assert not hasattr(live_test, "cleanup_namespace")
    assert not hasattr(CuaSandbox, "owns_namespace")
    assert not hasattr(FleetCloudTransport, "owns_namespace")


def test_selected_namespace_rejects_unsafe_override(monkeypatch) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "shared-production")

    with pytest.raises(ValueError, match="must start with cua-live-"):
        live_test.selected_namespace()


@pytest.mark.asyncio
async def test_existing_safe_namespace_never_provisions_or_deletes(monkeypatch) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    calls: list[str] = []

    class FakeHttpClient:
        async def aclose(self) -> None:
            calls.append("close")

    async def namespace_exists(fleet, namespace: str) -> bool:
        calls.append(f"exists:{namespace}")
        return True

    def ephemeral(*args, **kwargs):
        calls.append("provision")
        raise AssertionError("pre-existing namespace must not be provisioned")

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-existing")
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (object(), FakeHttpClient()))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test.Sandbox, "ephemeral", ephemeral)
    monkeypatch.setattr(live_test, "write_summary", lambda path, summary: None)

    with pytest.raises(RuntimeError, match="already exists"):
        await live_test.run_fleet_ephemeral_live()

    assert calls == ["exists:cua-live-existing", "close"]


@pytest.mark.asyncio
@pytest.mark.parametrize("cleanup_stage", ["poll", "inventory"])
async def test_cleanup_failure_does_not_mask_primary_failure(
    monkeypatch, cleanup_stage: str
) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    class PrimaryFailure(Exception):
        pass

    class CleanupFailure(Exception):
        pass

    class FakeSandbox:
        name = "cua-live-primary-failure"

    class FailingEphemeral:
        async def __aenter__(self):
            return FakeSandbox()

        async def __aexit__(self, exc_type, exc_value, traceback) -> None:
            return None

    class FakeFleet:
        async def get_template(self, namespace: str, name: str):
            raise PrimaryFailure("provision failed")

    class FakeHttpClient:
        async def aclose(self) -> None:
            return None

    summaries = []

    async def namespace_exists(fleet, namespace: str) -> bool:
        return False

    async def wait_namespace_absent(fleet, namespace: str) -> bool:
        if cleanup_stage == "poll":
            raise CleanupFailure("poll failed")
        return False

    async def collect_resource_inventory(fleet, namespace: str):
        raise CleanupFailure("inventory failed")

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-primary-failure")
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (FakeFleet(), FakeHttpClient()))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test.Sandbox, "ephemeral", lambda *args, **kwargs: FailingEphemeral())
    monkeypatch.setattr(live_test, "wait_namespace_absent", wait_namespace_absent)
    monkeypatch.setattr(live_test, "collect_resource_inventory", collect_resource_inventory)
    monkeypatch.setattr(live_test, "write_summary", lambda path, summary: summaries.append(summary))

    with pytest.raises(PrimaryFailure, match="provision failed"):
        await live_test.run_fleet_ephemeral_live()

    assert summaries[-1]["cleanup_error"] == {"type": "CleanupFailure"}


@pytest.mark.asyncio
async def test_summary_write_failure_still_closes_and_preserves_primary_error(monkeypatch) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    class SummaryWriteFailure(Exception):
        pass

    class FakeHttpClient:
        closed = False

        async def aclose(self) -> None:
            self.closed = True

    http_client = FakeHttpClient()

    async def namespace_exists(fleet, namespace: str) -> bool:
        return True

    def write_summary(path, summary) -> None:
        raise SummaryWriteFailure("disk unavailable")

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-summary-failure")
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (object(), http_client))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test, "write_summary", write_summary)

    with pytest.raises(RuntimeError, match="already exists"):
        await live_test.run_fleet_ephemeral_live()

    assert http_client.closed


@pytest.mark.asyncio
async def test_close_failure_does_not_mask_primary_error(monkeypatch) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    class CloseFailure(Exception):
        pass

    class FakeHttpClient:
        async def aclose(self) -> None:
            raise CloseFailure("close failed")

    summaries = []

    async def namespace_exists(fleet, namespace: str) -> bool:
        return True

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-close-failure")
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (object(), FakeHttpClient()))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test, "write_summary", lambda path, summary: summaries.append(summary))

    with pytest.raises(RuntimeError, match="already exists"):
        await live_test.run_fleet_ephemeral_live()

    assert summaries[-1]["close_error"] == {"type": "CloseFailure"}


@pytest.mark.parametrize(
    "namespace",
    ["cua-live-UPPER", "cua-live-", "cua-live-" + "a" * 55, "cua-live-name_"],
)
def test_selected_namespace_requires_dns_1123_label(monkeypatch, namespace: str) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", namespace)

    with pytest.raises(ValueError, match="DNS-1123 label"):
        live_test.selected_namespace()


@pytest.mark.asyncio
async def test_raced_attached_namespace_leak_records_inventory_without_deletion(
    monkeypatch, tmp_path
) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    summaries = []

    class FakeSandbox:
        name = "cua-live-raced"

        class screen:
            @staticmethod
            async def size():
                return (1024, 768)

        @staticmethod
        async def screenshot():
            return b"\x89PNG\r\n\x1a\n" + b"x" * 1001

        class shell:
            @staticmethod
            async def run(command: str):
                return SimpleNamespace(success=True, stdout="Linux\n", stderr="")

    class FakeEphemeral:
        async def __aenter__(self):
            return FakeSandbox()

        async def __aexit__(self, exc_type, exc_value, traceback) -> None:
            return None

    class FakeHttpClient:
        async def aclose(self) -> None:
            return None

    class FakeFleet:
        async def get_template(self, namespace: str, name: str):
            return object()

    async def namespace_exists(fleet, namespace: str) -> bool:
        return False

    async def wait_namespace_absent(fleet, namespace: str) -> bool:
        return False

    async def collect_resource_inventory(fleet, namespace: str):
        return {"templates": ["raced-template"], "pools": [], "claims": []}
    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-raced")
    monkeypatch.setenv("CUA_LIVE_E2E_ARTIFACT_DIR", str(tmp_path))
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (FakeFleet(), FakeHttpClient()))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test.Sandbox, "ephemeral", lambda *args, **kwargs: FakeEphemeral())
    monkeypatch.setattr(live_test, "assert_template_contract", lambda template, expected_port: None)
    monkeypatch.setattr(live_test, "wait_namespace_absent", wait_namespace_absent)
    monkeypatch.setattr(live_test, "collect_resource_inventory", collect_resource_inventory)
    monkeypatch.setattr(live_test, "write_summary", lambda path, summary: summaries.append(summary))

    with pytest.raises(pytest.fail.Exception, match="leaked"):
        await live_test.run_fleet_ephemeral_live()

    assert summaries[-1]["namespace_leak"] is True
    assert summaries[-1]["remaining_resources"] == {
        "templates": ["raced-template"],
        "pools": [],
        "claims": [],
    }


@pytest.mark.asyncio
async def test_provisioning_failure_before_yield_never_deletes_namespace(monkeypatch) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    class ProvisioningFailure(Exception):
        pass

    summaries = []

    class FailingEphemeral:
        async def __aenter__(self):
            raise ProvisioningFailure("provisioning failed")

        async def __aexit__(self, exc_type, exc_value, traceback) -> None:
            return None

    class FakeHttpClient:
        async def aclose(self) -> None:
            return None

    async def namespace_exists(fleet, namespace: str) -> bool:
        return False

    async def wait_namespace_absent(fleet, namespace: str) -> bool:
        return False

    async def collect_resource_inventory(fleet, namespace: str):
        return {"templates": [], "pools": [], "claims": []}

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-provisioning-failure")
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (object(), FakeHttpClient()))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test.Sandbox, "ephemeral", lambda *args, **kwargs: FailingEphemeral())
    monkeypatch.setattr(live_test, "wait_namespace_absent", wait_namespace_absent)
    monkeypatch.setattr(live_test, "collect_resource_inventory", collect_resource_inventory)
    monkeypatch.setattr(live_test, "write_summary", lambda path, summary: summaries.append(summary))

    with pytest.raises(ProvisioningFailure, match="provisioning failed"):
        await live_test.run_fleet_ephemeral_live()

    assert summaries[-1]["provisioning"] == {"sandbox_yielded": False}


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("close_fails", "summary_fails"),
    [(True, False), (False, True), (True, True)],
)
async def test_cleanup_error_precedes_close_and_summary_failures(
    monkeypatch, tmp_path, close_fails: bool, summary_fails: bool
) -> None:
    from tests.live import test_fleet_ephemeral as live_test

    class CloseFailure(Exception):
        pass

    class SummaryFailure(Exception):
        pass

    summaries = []

    class FakeSandbox:
        name = "cua-live-owned"

        class screen:
            @staticmethod
            async def size():
                return (1024, 768)

        @staticmethod
        async def screenshot():
            return b"\x89PNG\r\n\x1a\n" + b"x" * 1001

        class shell:
            @staticmethod
            async def run(command: str):
                return SimpleNamespace(success=True, stdout="Linux\n", stderr="")

    class FakeEphemeral:
        async def __aenter__(self):
            return FakeSandbox()

        async def __aexit__(self, exc_type, exc_value, traceback) -> None:
            return None

    class FakeFleet:
        async def get_template(self, namespace: str, name: str):
            return object()

    class FakeHttpClient:
        async def aclose(self) -> None:
            if close_fails:
                raise CloseFailure("close failed")

    async def namespace_exists(fleet, namespace: str) -> bool:
        return False

    async def wait_namespace_absent(fleet, namespace: str) -> bool:
        return False

    async def collect_resource_inventory(fleet, namespace: str):
        return {"templates": ["owned-template"], "pools": [], "claims": []}

    def write_summary(path, summary) -> None:
        summaries.append(summary)
        if summary_fails:
            raise SummaryFailure("summary failed")

    monkeypatch.setenv("CUA_LIVE_E2E_NAMESPACE", "cua-live-owned")
    monkeypatch.setenv("CUA_LIVE_E2E_ARTIFACT_DIR", str(tmp_path))
    monkeypatch.setattr(live_test, "build_fleet_client", lambda: (FakeFleet(), FakeHttpClient()))
    monkeypatch.setattr(live_test, "namespace_exists", namespace_exists)
    monkeypatch.setattr(live_test.Sandbox, "ephemeral", lambda *args, **kwargs: FakeEphemeral())
    monkeypatch.setattr(live_test, "assert_template_contract", lambda template, expected_port: None)
    monkeypatch.setattr(live_test, "wait_namespace_absent", wait_namespace_absent)
    monkeypatch.setattr(live_test, "collect_resource_inventory", collect_resource_inventory)
    monkeypatch.setattr(live_test, "write_summary", write_summary)

    with pytest.raises(pytest.fail.Exception, match="leaked"):
        await live_test.run_fleet_ephemeral_live()

    assert summaries[-1]["cleanup_error"] == {"type": "Failed"}
    if close_fails:
        assert summaries[-1]["close_error"] == {"type": "CloseFailure"}
    if summary_fails:
        assert summaries[-1]["summary_error"] == {"type": "SummaryFailure"}
