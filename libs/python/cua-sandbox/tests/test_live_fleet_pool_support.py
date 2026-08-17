from __future__ import annotations

from types import SimpleNamespace

import pytest
from fleet_sdk import SdkError

from tests.live import test_fleet_pool_persistent as pool_live

PNG = b"\x89PNG\r\n\x1a\n" + b"0" * 2000


def make_pool_resource(namespace: str, *, spec_replicas: int = 1, ready_replicas: int = 1):
    return SimpleNamespace(
        metadata=SimpleNamespace(namespace=namespace, name=namespace),
        spec=SimpleNamespace(replicas=spec_replicas),
        status=SimpleNamespace(ready_replicas=ready_replicas),
    )


def not_found(operation: str = "get pool") -> SdkError.Status:
    return SdkError.Status(operation, 404, b"not found")


class FakePoolHandle:
    def __init__(self, resource) -> None:
        self._resource = resource

    @property
    def name(self) -> str:
        return self._resource.metadata.name

    @property
    def resource(self):
        return self._resource


class FakeSandbox:
    def __init__(self, name: str, shell_stdout: str) -> None:
        self.name = name
        self.claim_name = name
        self.pool_name = name
        self.screen = SimpleNamespace(size=self._size)
        self.shell = SimpleNamespace(run=self._run)
        self._shell_stdout = shell_stdout

    async def _size(self) -> tuple[int, int]:
        return (1024, 768)

    async def _run(self, command: str):
        return SimpleNamespace(success=True, stdout=self._shell_stdout, stderr="")

    async def screenshot(self) -> bytes:
        return PNG


class _FakeEphemeral:
    def __init__(self, sandbox: FakeSandbox) -> None:
        self._sandbox = sandbox

    async def __aenter__(self) -> FakeSandbox:
        return self._sandbox

    async def __aexit__(self, exc_type, exc, traceback) -> bool:
        return False


class PoolRunnerHarness:
    def __init__(self) -> None:
        self.get_results: list = []
        self.apply_calls: list[dict] = []
        self.apply_error: BaseException | None = None
        self.ephemeral_calls: list[dict] = []
        self.shell_stdout = "Linux"
        self.claims_absent: object = True
        self.claims_absent_calls: list[str] = []
        self.inventory: object = None
        self.inventory_calls: list[str] = []
        self.template_contract_calls: list[tuple] = []
        self.summaries: dict[str, dict] = {}


def install_pool_runner(monkeypatch, tmp_path, *, mode: str, namespace: str) -> PoolRunnerHarness:
    harness = PoolRunnerHarness()
    monkeypatch.setenv("CUA_LIVE_E2E_LANE", "test")
    monkeypatch.setenv("CUA_LIVE_E2E_EVENT", "schedule")
    monkeypatch.setenv("CUA_LIVE_E2E_ARTIFACT_DIR", str(tmp_path))
    monkeypatch.setenv(pool_live.MODE_ENV[mode], namespace)

    class FakePool:
        @staticmethod
        async def get(name: str) -> FakePoolHandle:
            assert harness.get_results, "unexpected Pool.get call"
            result = harness.get_results.pop(0)
            if isinstance(result, BaseException):
                raise result
            return FakePoolHandle(result)

        @staticmethod
        async def apply(image, **kwargs) -> FakePoolHandle:
            harness.apply_calls.append({"image": image, **kwargs})
            if harness.apply_error is not None:
                raise harness.apply_error
            return FakePoolHandle(make_pool_resource(kwargs["name"]))

    class FakeSandboxApi:
        @staticmethod
        def ephemeral(**kwargs) -> _FakeEphemeral:
            harness.ephemeral_calls.append(kwargs)
            return _FakeEphemeral(FakeSandbox(kwargs["name"], harness.shell_stdout))

    class FakeFleet:
        async def get_template(self, template_namespace: str, name: str):
            return SimpleNamespace(namespace=template_namespace, name=name)

    class FakeHttpClient:
        async def aclose(self) -> None:
            return None

    async def fake_wait_claims_absent(fleet, name: str) -> bool:
        harness.claims_absent_calls.append(name)
        if isinstance(harness.claims_absent, BaseException):
            raise harness.claims_absent
        return bool(harness.claims_absent)

    async def fake_collect_resource_inventory(fleet, name: str):
        harness.inventory_calls.append(name)
        if isinstance(harness.inventory, BaseException):
            raise harness.inventory
        if harness.inventory is None:
            return {"templates": [name], "pools": [name], "claims": []}
        return harness.inventory

    def fake_assert_template_contract(template, expected_port: int) -> None:
        harness.template_contract_calls.append((template, expected_port))

    def fake_write_summary(path, summary) -> None:
        harness.summaries[path.name] = summary

    monkeypatch.setattr(pool_live, "Pool", FakePool)
    monkeypatch.setattr(pool_live, "Sandbox", FakeSandboxApi)
    monkeypatch.setattr(pool_live, "build_fleet_client", lambda: (FakeFleet(), FakeHttpClient()))
    monkeypatch.setattr(pool_live, "wait_claims_absent", fake_wait_claims_absent)
    monkeypatch.setattr(pool_live, "collect_resource_inventory", fake_collect_resource_inventory)
    monkeypatch.setattr(pool_live, "assert_template_contract", fake_assert_template_contract)
    monkeypatch.setattr(pool_live, "write_summary", fake_write_summary)
    return harness


@pytest.mark.asyncio
async def test_pool_live_runner_uses_pinned_warm_configuration(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [
        make_pool_resource(namespace, ready_replicas=1),
        make_pool_resource(namespace, ready_replicas=1),
    ]

    await pool_live.run_fleet_pool_live("warm")

    (apply_call,) = harness.apply_calls
    assert apply_call["name"] == namespace
    assert apply_call["replicas"] == 1
    assert apply_call["cpu"] == 4
    assert apply_call["memory_mb"] == 4096
    assert apply_call["autoscaling"] is None

    (ephemeral_call,) = harness.ephemeral_calls
    assert ephemeral_call["pool"] == namespace
    assert isinstance(ephemeral_call["pool"], str)
    assert ephemeral_call["name"] == namespace
    assert ephemeral_call["time_to_start"] == 180
    assert ephemeral_call["telemetry_enabled"] is False
    for rejected in ("image", "cpu", "memory_mb", "server_port", "replicas"):
        assert rejected not in ephemeral_call

    ((_, contract_port),) = harness.template_contract_calls
    assert contract_port == 8000
    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["pool_pre_existed"] is True
    assert summary["ready_replicas_before"] == 1
    assert summary["warm_bind_sla_applied"] is True
    assert summary["claims_absent"] is True
    assert summary["persistent_resources"] == {
        "templates": [namespace],
        "pools": [namespace],
        "claims": [],
    }
    assert summary["spec_replicas_after"] == 1


@pytest.mark.asyncio
async def test_pool_live_runner_uses_pinned_cold_configuration(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-cold-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="cold", namespace=namespace)
    harness.get_results = [
        make_pool_resource(namespace, ready_replicas=0),
        make_pool_resource(namespace, ready_replicas=0),
    ]

    await pool_live.run_fleet_pool_live("cold")

    (apply_call,) = harness.apply_calls
    assert apply_call["replicas"] == 1
    autoscaling = apply_call["autoscaling"]
    assert autoscaling is not None
    assert autoscaling.min_pool_size == 0
    assert autoscaling.initial_pool_size == 0
    assert autoscaling.max_pool_size == 1

    (ephemeral_call,) = harness.ephemeral_calls
    assert ephemeral_call["time_to_start"] == 900

    summary = harness.summaries["summary-pool-cold.json"]
    assert summary["warm_bind_sla_applied"] is False


@pytest.mark.asyncio
async def test_pool_pre_existed_false_is_recorded_for_public_sdk_404(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [not_found(), make_pool_resource(namespace)]

    await pool_live.run_fleet_pool_live("warm")

    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["pool_pre_existed"] is False
    assert "ready_replicas_before" not in summary
    assert summary["warm_bind_sla_applied"] is False


@pytest.mark.asyncio
async def test_empty_inventory_fails_as_unexpected(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [make_pool_resource(namespace), make_pool_resource(namespace)]
    harness.inventory = {"templates": [], "pools": [], "claims": []}

    with pytest.raises(pytest.fail.Exception, match="persistent pool inventory"):
        await pool_live.run_fleet_pool_live("warm")

    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["unexpected_inventory"] is True


@pytest.mark.asyncio
async def test_claim_leak_fails_and_is_recorded(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [make_pool_resource(namespace), make_pool_resource(namespace)]
    harness.claims_absent = False

    with pytest.raises(pytest.fail.Exception, match="claims remain"):
        await pool_live.run_fleet_pool_live("warm")

    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["claim_leak"] is True


@pytest.mark.asyncio
async def test_warm_sla_not_applied_without_ready_replicas(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [
        make_pool_resource(namespace, ready_replicas=0),
        make_pool_resource(namespace, ready_replicas=1),
    ]
    monkeypatch.setattr(pool_live, "WARM_BIND_SLA_SECONDS", -1.0)

    await pool_live.run_fleet_pool_live("warm")

    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["warm_bind_sla_applied"] is False
    assert "error" not in summary


@pytest.mark.asyncio
async def test_warm_sla_enforced_with_ready_replicas(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [
        make_pool_resource(namespace, ready_replicas=1),
        make_pool_resource(namespace, ready_replicas=1),
    ]
    monkeypatch.setattr(pool_live, "WARM_BIND_SLA_SECONDS", -1.0)

    with pytest.raises(AssertionError, match="pre-provisioned warm claim"):
        await pool_live.run_fleet_pool_live("warm")

    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["warm_bind_sla_applied"] is True
    assert summary["error"] == {"type": "AssertionError"}


@pytest.mark.asyncio
async def test_cleanup_failure_does_not_mask_primary_failure(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [make_pool_resource(namespace), make_pool_resource(namespace)]
    harness.shell_stdout = "Darwin"
    harness.inventory = RuntimeError("inventory unavailable")

    with pytest.raises(AssertionError):
        await pool_live.run_fleet_pool_live("warm")

    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["error"] == {"type": "AssertionError"}
    assert summary["cleanup_error"] == {"type": "RuntimeError"}


@pytest.mark.asyncio
async def test_apply_failure_skips_persistence_verification(monkeypatch, tmp_path) -> None:
    namespace = "cua-live-pool-warm-test-schedule"
    harness = install_pool_runner(monkeypatch, tmp_path, mode="warm", namespace=namespace)
    harness.get_results = [not_found()]
    harness.apply_error = RuntimeError("apply rejected")

    with pytest.raises(RuntimeError, match="apply rejected"):
        await pool_live.run_fleet_pool_live("warm")

    assert harness.claims_absent_calls == []
    assert harness.inventory_calls == []
    assert harness.ephemeral_calls == []
    summary = harness.summaries["summary-pool-warm.json"]
    assert summary["provisioning"] == {"pool_applied": False, "sandbox_yielded": False}
    assert summary["error"] == {"type": "RuntimeError"}


def test_selected_pool_namespace_rejects_foreign_prefix(monkeypatch) -> None:
    monkeypatch.setenv("CUA_LIVE_E2E_POOL_WARM_NAMESPACE", "cua-live-warm-other")

    with pytest.raises(ValueError, match="must start with cua-live-pool-warm-"):
        pool_live.selected_pool_namespace("warm")


def test_selected_pool_namespace_requires_dns_1123_label(monkeypatch) -> None:
    monkeypatch.setenv("CUA_LIVE_E2E_POOL_COLD_NAMESPACE", "cua-live-pool-cold-" + "x" * 60)

    with pytest.raises(ValueError, match="DNS-1123"):
        pool_live.selected_pool_namespace("cold")


def test_selected_pool_namespace_builds_default_from_lane_and_event(monkeypatch) -> None:
    monkeypatch.delenv("CUA_LIVE_E2E_POOL_WARM_NAMESPACE", raising=False)
    monkeypatch.setenv("CUA_LIVE_E2E_LANE", "main-source")
    monkeypatch.setenv("CUA_LIVE_E2E_EVENT", "schedule")

    assert pool_live.selected_pool_namespace("warm") == "cua-live-pool-warm-main-source-schedule"
