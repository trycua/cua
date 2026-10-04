from __future__ import annotations

import asyncio
import os
import re
import time
from importlib.metadata import version
from pathlib import Path

import cua_sandbox
import pytest
from cua_sandbox import Image, Sandbox

from tests.live.fleet_e2e_support import (
    KUBEVIRT_ENV_SKIP,
    assert_env_template_contract,
    assert_template_contract,
    build_fleet_client,
    build_namespace_name,
    check_service_reachable,
    check_spacesd,
    collect_resource_inventory,
    env_image,
    namespace_prefix_ok,
    wait_claims_absent,
    write_summary,
)

# Pinned image with its own daemon on 8000: the daemon-agnostic lane declares
# it as server_port and checks only lifecycle + service reachability.
IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2"
)


INVENTORY_SETTLE_SECONDS = float(os.environ.get("CUA_LIVE_E2E_INVENTORY_SETTLE", "240"))


def has_oauth_credentials() -> bool:
    return bool(os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET"))


def selected_namespace() -> str:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = os.environ.get("CUA_LIVE_E2E_NAMESPACE") or build_namespace_name(
        lane,
        os.environ.get("CUA_LIVE_E2E_EVENT", os.environ.get("GITHUB_EVENT_NAME", "manual")),
    )
    if not namespace_prefix_ok(namespace, "cua-live-", "cua-e2e-"):
        raise ValueError("CUA_LIVE_E2E_NAMESPACE must start with cua-live- or cua-e2e-")
    if len(namespace) > 63 or re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", namespace) is None:
        raise ValueError("CUA_LIVE_E2E_NAMESPACE must be a DNS-1123 label of at most 63 characters")
    return namespace


async def delete_managed_pool(name: str) -> None:
    """GC this lane's managed pool through the SDK: only this pool, and only
    once it has no claims (another run sharing the spec keeps it)."""
    from cua_sandbox import _autopool

    report = await _autopool.gc_pools([name], 0)
    if report.errors:
        raise RuntimeError(f"managed pool GC failed for {name}: {report.errors}")


pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(not has_oauth_credentials(), reason="Fleet OAuth credentials not set"),
]


async def run_fleet_ephemeral_live() -> None:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = selected_namespace()
    artifact_dir = Path(os.environ.get("CUA_LIVE_E2E_ARTIFACT_DIR", "/tmp/cua-live-e2e"))
    spacesd_image = env_image()
    summary = {
        "lane": lane,
        "namespace": namespace,
        "image": spacesd_image or IMAGE,
        "guest_daemon": "cua-spacesd" if spacesd_image else "image-provided server:8000",
        "source_sha": os.environ.get("CUA_LIVE_E2E_SOURCE_SHA") or os.environ.get("GITHUB_SHA"),
        "packages": {
            "cua-sandbox": version("cua-sandbox"),
            "cua-fleet": version("cua-fleet"),
            "cua": version("cua"),
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
    provisioning_attempted = False
    sandbox_yielded = False

    def record_cleanup_error(error: BaseException) -> None:
        nonlocal cleanup_error
        error_summary = {"type": type(error).__name__}
        if cleanup_error is None:
            cleanup_error = error
            summary["cleanup_error"] = error_summary
        else:
            summary.setdefault("cleanup_secondary_errors", []).append(error_summary)

    # Sandbox.ephemeral(image) claims from a managed cua-auto-<spec hash>
    # pool. Cleanup below GCs exactly that pool by name (only once it has no
    # claims), and automatic GC stays away from other pools.
    managed_prefix = "cua-auto-"
    saved_env = {key: os.environ.get(key) for key in ("CUA_FLEET_POOL_IDLE_GC",)}
    os.environ["CUA_FLEET_POOL_IDLE_GC"] = "off"
    try:
        provisioning_attempted = True
        started = time.monotonic()
        options = {
            "name": namespace,
            "cpu": 4,
            "memory_mb": 4096,
            "time_to_start": 900,
            "telemetry_enabled": False,
        }
        if spacesd_image is None:
            options["server_port"] = 8000
        async with Sandbox.ephemeral(
            Image.from_registry(spacesd_image or IMAGE), **options, local=False
        ) as sandbox:
            sandbox_yielded = True
            summary["provision_seconds"] = time.monotonic() - started
            summary["sandbox_name"] = sandbox.name
            sandbox_claim_name = getattr(sandbox, "claim_name", None)
            sandbox_pool_name = getattr(sandbox, "pool_name", None)
            claim_name = sandbox_claim_name or sandbox.name
            pool_name = sandbox_pool_name or namespace
            summary["claim_name"] = claim_name
            summary["pool_name"] = pool_name
            try:
                assert (
                    isinstance(sandbox.name, str) and sandbox.name
                ), "sandbox name must be a non-empty string"
                if sandbox_claim_name is not None:
                    assert (
                        claim_name == namespace
                    ), f"claim name {claim_name!r} must equal requested name {namespace!r}"
                if sandbox_pool_name is not None:
                    assert pool_name.startswith(
                        managed_prefix
                    ), f"pool name {pool_name!r} must be a managed pool ({managed_prefix}*)"

                template = await fleet.get_template(pool_name, pool_name)
                if spacesd_image is None:
                    assert_template_contract(template, expected_port=8000)
                    await check_service_reachable(sandbox, "server", summary)
                else:
                    assert_env_template_contract(template)
                    await check_spacesd(sandbox, summary, artifact_dir, "ephemeral")

                if os.environ.get("CUA_LIVE_E2E_SIGNED_URLS") == "true":
                    signed_service = "server" if spacesd_image is None else "env"
                    signed_url = await sandbox.services.create_signed_url(
                        signed_service,
                        label="periodic-live-e2e",
                        expires_in_seconds=300,
                    )
                    assert signed_url.namespace == pool_name
                    assert signed_url.service == signed_service
                    assert signed_url.label == "periodic-live-e2e"
                    assert signed_url.revoked_at is None

                    try:
                        listed_signed_urls = await sandbox.services.list_signed_urls()
                        listed_signed_url = next(
                            item for item in listed_signed_urls if item.id == signed_url.id
                        )
                        assert listed_signed_url.revoked_at is None
                    finally:
                        await sandbox.services.revoke_signed_url(signed_url)

                    revoked_signed_urls = await sandbox.services.list_signed_urls()
                    revoked_signed_url = next(
                        item for item in revoked_signed_urls if item.id == signed_url.id
                    )
                    assert revoked_signed_url.revoked_at is not None
                    summary["signed_service_url"] = {
                        "created": True,
                        "listed": True,
                        "revoked": True,
                    }
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
        if provisioning_attempted:
            cleanup_started = time.monotonic()
            claims_absent: bool | None = None
            inventory: dict[str, list[str]] | None = None
            resource_namespace = summary.get("pool_name")
            if resource_namespace is not None:
                try:
                    claims_absent = await wait_claims_absent(fleet, resource_namespace)
                    summary["claims_absent"] = claims_absent
                except BaseException as error:
                    record_cleanup_error(error)
                if claims_absent and str(resource_namespace).startswith(managed_prefix):
                    # The managed pool outlives the ephemeral claim by design
                    # (it is reused); this lane owns it, so remove it now.
                    try:
                        await delete_managed_pool(resource_namespace)
                    except BaseException as error:
                        record_cleanup_error(error)
                try:
                    expected_inventory = {"templates": [], "pools": [], "claims": []}
                    inventory = await collect_resource_inventory(fleet, resource_namespace)
                    # Pool deletion tears the namespace down asynchronously:
                    # give it a bounded time to settle before calling it a leak.
                    settle_deadline = time.monotonic() + INVENTORY_SETTLE_SECONDS
                    while (
                        claims_absent
                        and inventory != {"templates": [], "pools": [], "claims": []}
                        and time.monotonic() < settle_deadline
                    ):
                        await asyncio.sleep(5)
                        inventory = await collect_resource_inventory(fleet, resource_namespace)
                    summary["persistent_resources"] = inventory
                except BaseException as error:
                    record_cleanup_error(error)

            if claims_absent is False:
                try:
                    summary["claim_leak"] = True
                    pytest.fail(
                        f"claims remain in namespace {namespace} after Sandbox.ephemeral(local=False)"
                    )
                except BaseException as error:
                    record_cleanup_error(error)
            if sandbox_yielded and inventory is not None:
                if inventory != expected_inventory:
                    try:
                        summary["unexpected_inventory"] = True
                        pytest.fail(
                            "unexpected reconciled resource inventory "
                            f"for namespace {namespace}: {inventory}"
                        )
                    except BaseException as error:
                        record_cleanup_error(error)
            summary["cleanup_seconds"] = time.monotonic() - cleanup_started
            if not sandbox_yielded:
                summary["provisioning"] = {"attempted": True, "sandbox_yielded": False}

        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        try:
            await http_client.aclose()
        except BaseException as error:
            close_error = error
            summary["close_error"] = {"type": type(error).__name__}

        try:
            write_summary(artifact_dir / "summary.json", summary)
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


async def test_fleet_ephemeral_live() -> None:
    if env_image() is not None:
        pytest.skip(KUBEVIRT_ENV_SKIP)
    await run_fleet_ephemeral_live()
