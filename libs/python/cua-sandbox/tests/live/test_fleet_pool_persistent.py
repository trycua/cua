from __future__ import annotations

import os
import re
import time
from importlib.metadata import version
from pathlib import Path

import cua_sandbox
import pytest
from cua_sandbox import Image, Pool, Sandbox, WarmPoolAutoscaling

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_fleet_client,
    build_pool_namespace_name,
    collect_resource_inventory,
    has_oauth_credentials,
    is_pool_missing_error,
    wait_claims_absent,
    write_summary,
)

IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2"
)

POOL_CPU = 4
POOL_MEMORY_MB = 4096
WARM_TIME_TO_START = 180
COLD_TIME_TO_START = 900
WARM_BIND_SLA_SECONDS = 300

MODE_ENV = {
    "warm": "CUA_LIVE_E2E_POOL_WARM_NAMESPACE",
    "cold": "CUA_LIVE_E2E_POOL_COLD_NAMESPACE",
}


def cold_autoscaling() -> WarmPoolAutoscaling:
    # replicas < 1 is rejected by Pool.apply, so scale-to-zero is expressed
    # through autoscaling: KEDA owns spec.replicas and decays it to zero.
    return WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=0, max_pool_size=1)


def selected_pool_namespace(mode: str) -> str:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = os.environ.get(MODE_ENV[mode]) or build_pool_namespace_name(
        mode,
        lane,
        os.environ.get("CUA_LIVE_E2E_EVENT", os.environ.get("GITHUB_EVENT_NAME", "manual")),
    )
    if not namespace.startswith(f"cua-live-pool-{mode}-"):
        raise ValueError(f"{MODE_ENV[mode]} must start with cua-live-pool-{mode}-")
    if len(namespace) > 63 or re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", namespace) is None:
        raise ValueError(f"{MODE_ENV[mode]} must be a DNS-1123 label of at most 63 characters")
    return namespace


pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(not has_oauth_credentials(), reason="Fleet OAuth credentials not set"),
]


async def run_fleet_pool_live(mode: str) -> None:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = selected_pool_namespace(mode)
    artifact_dir = Path(os.environ.get("CUA_LIVE_E2E_ARTIFACT_DIR", "/tmp/cua-live-e2e"))
    summary = {
        "lane": lane,
        "mode": mode,
        "namespace": namespace,
        "image": IMAGE,
        "source_sha": os.environ.get("CUA_LIVE_E2E_SOURCE_SHA") or os.environ.get("GITHUB_SHA"),
        "packages": {
            "cua-sandbox": version("cua-sandbox"),
            "cua-fleet": version("cua-fleet"),
        },
        "module_origins": {
            "cua_sandbox": str(Path(cua_sandbox.__file__).resolve()),
        },
    }
    fleet, http_client = build_fleet_client()
    primary_error: BaseException | None = None
    cleanup_error: BaseException | None = None
    close_error: BaseException | None = None
    summary_error: BaseException | None = None
    pool_applied = False
    sandbox_yielded = False

    def record_cleanup_error(error: BaseException) -> None:
        nonlocal cleanup_error
        error_summary = {"type": type(error).__name__}
        if cleanup_error is None:
            cleanup_error = error
            summary["cleanup_error"] = error_summary
        else:
            summary.setdefault("cleanup_secondary_errors", []).append(error_summary)

    try:
        try:
            existing = await Pool.get(namespace)
        except BaseException as error:
            if not is_pool_missing_error(error):
                raise
            summary["pool_pre_existed"] = False
        else:
            summary["pool_pre_existed"] = True
            summary["spec_replicas_before"] = existing.resource.spec.replicas
            status = getattr(existing.resource, "status", None)
            summary["ready_replicas_before"] = (
                (getattr(status, "ready_replicas", None) or 0) if status is not None else 0
            )

        apply_started = time.monotonic()
        pool = await Pool.apply(
            Image.from_registry(IMAGE),
            name=namespace,
            replicas=1,
            cpu=POOL_CPU,
            memory_mb=POOL_MEMORY_MB,
            autoscaling=None if mode == "warm" else cold_autoscaling(),
        )
        pool_applied = True
        summary["apply_seconds"] = time.monotonic() - apply_started
        assert pool.name == namespace, f"pool name {pool.name!r} must equal namespace {namespace!r}"

        template = await fleet.get_template(namespace, namespace)
        assert_template_contract(template, expected_port=8000)

        claim_started = time.monotonic()
        async with Sandbox.ephemeral(
            pool=namespace,
            name=namespace,
            time_to_start=WARM_TIME_TO_START if mode == "warm" else COLD_TIME_TO_START,
            telemetry_enabled=False,
        ) as sandbox:
            sandbox_yielded = True
            claim_seconds = time.monotonic() - claim_started
            summary["claim_seconds"] = claim_seconds
            summary["sandbox_name"] = sandbox.name
            sandbox_claim_name = getattr(sandbox, "claim_name", None)
            sandbox_pool_name = getattr(sandbox, "pool_name", None)
            summary["claim_name"] = sandbox_claim_name or sandbox.name
            summary["pool_name"] = sandbox_pool_name or namespace
            try:
                assert (
                    isinstance(sandbox.name, str) and sandbox.name
                ), "sandbox name must be a non-empty string"
                if sandbox_claim_name is not None:
                    assert sandbox_claim_name == namespace, (
                        f"claim name {sandbox_claim_name!r} must equal "
                        f"requested name {namespace!r}"
                    )
                if sandbox_pool_name is not None:
                    assert (
                        sandbox_pool_name == namespace
                    ), f"pool name {sandbox_pool_name!r} must equal namespace {namespace!r}"

                warm_bind_sla_applied = (
                    mode == "warm"
                    and summary.get("pool_pre_existed") is True
                    and summary.get("ready_replicas_before", 0) >= 1
                )
                summary["warm_bind_sla_applied"] = warm_bind_sla_applied
                if warm_bind_sla_applied:
                    assert claim_seconds < WARM_BIND_SLA_SECONDS, (
                        f"pre-provisioned warm claim took {claim_seconds:.1f}s, "
                        f"expected under {WARM_BIND_SLA_SECONDS}s"
                    )

                width, height = await sandbox.screen.size()
                summary["screen"] = {"width": width, "height": height}
                assert (width, height) == (1024, 768)

                screenshot = await sandbox.screenshot()
                artifact_dir.mkdir(parents=True, exist_ok=True)
                (artifact_dir / f"screen-pool-{mode}.png").write_bytes(screenshot)
                assert screenshot.startswith(b"\x89PNG\r\n\x1a\n")
                assert len(screenshot) > 1000

                result = await sandbox.shell.run("uname -s")
                summary["shell"] = {
                    "success": result.success,
                    "stdout": result.stdout.strip(),
                    "stderr": result.stderr.strip(),
                }
                assert result.success
                assert result.stdout.strip() == "Linux"
            except BaseException as error:
                primary_error = error
                summary["error"] = {"type": type(error).__name__}
    except BaseException as error:
        if primary_error is None:
            if sandbox_yielded:
                record_cleanup_error(error)
            else:
                primary_error = error
                summary["error"] = {"type": type(error).__name__}
        else:
            summary["context_exit_error"] = {"type": type(error).__name__}
    finally:
        if pool_applied:
            cleanup_started = time.monotonic()
            claims_absent: bool | None = None
            inventory: dict[str, list[str]] | None = None
            try:
                claims_absent = await wait_claims_absent(fleet, namespace)
                summary["claims_absent"] = claims_absent
            except BaseException as error:
                record_cleanup_error(error)
            try:
                inventory = await collect_resource_inventory(fleet, namespace)
                summary["persistent_resources"] = inventory
            except BaseException as error:
                record_cleanup_error(error)

            if claims_absent is False:
                try:
                    summary["claim_leak"] = True
                    pytest.fail(f"claims remain in namespace {namespace} after claim-only release")
                except BaseException as error:
                    record_cleanup_error(error)
            if inventory is not None:
                expected_inventory = {
                    "templates": [namespace],
                    "pools": [namespace],
                    "claims": [],
                }
                if inventory != expected_inventory:
                    try:
                        summary["unexpected_inventory"] = True
                        pytest.fail(
                            f"persistent pool inventory for namespace {namespace} must be "
                            f"{expected_inventory}, got {inventory}"
                        )
                    except BaseException as error:
                        record_cleanup_error(error)
            try:
                refreshed = await Pool.get(namespace)
                summary["spec_replicas_after"] = refreshed.resource.spec.replicas
                status = getattr(refreshed.resource, "status", None)
                summary["ready_replicas_after"] = (
                    (getattr(status, "ready_replicas", None) or 0) if status is not None else 0
                )
            except BaseException as error:
                record_cleanup_error(error)
            summary["cleanup_seconds"] = time.monotonic() - cleanup_started
        if not sandbox_yielded:
            summary["provisioning"] = {
                "pool_applied": pool_applied,
                "sandbox_yielded": False,
            }

        try:
            await http_client.aclose()
        except BaseException as error:
            close_error = error
            summary["close_error"] = {"type": type(error).__name__}

        try:
            write_summary(artifact_dir / f"summary-pool-{mode}.json", summary)
        except BaseException as error:
            summary_error = error
            summary["summary_error"] = {"type": type(error).__name__}

    if primary_error is not None:
        raise primary_error
    if cleanup_error is not None:
        raise cleanup_error
    if close_error is not None:
        raise close_error
    if summary_error is not None:
        raise summary_error


async def test_fleet_pool_warm_live() -> None:
    await run_fleet_pool_live("warm")


async def test_fleet_pool_cold_live() -> None:
    await run_fleet_pool_live("cold")
