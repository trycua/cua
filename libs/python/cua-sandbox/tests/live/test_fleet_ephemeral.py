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
    assert_template_contract,
    build_fleet_client,
    build_namespace_name,
    collect_resource_inventory,
    wait_claims_absent,
    write_summary,
)

IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2"
)


def has_oauth_credentials() -> bool:
    return bool(os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET"))


def selected_namespace() -> str:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = os.environ.get("CUA_LIVE_E2E_NAMESPACE") or build_namespace_name(
        lane,
        os.environ.get("CUA_LIVE_E2E_EVENT", os.environ.get("GITHUB_EVENT_NAME", "manual")),
    )
    if not namespace.startswith("cua-live-"):
        raise ValueError("CUA_LIVE_E2E_NAMESPACE must start with cua-live-")
    if len(namespace) > 63 or re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?", namespace) is None:
        raise ValueError("CUA_LIVE_E2E_NAMESPACE must be a DNS-1123 label of at most 63 characters")
    return namespace


pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(not has_oauth_credentials(), reason="Fleet OAuth credentials not set"),
]


async def run_fleet_ephemeral_live() -> None:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = selected_namespace()
    artifact_dir = Path(os.environ.get("CUA_LIVE_E2E_ARTIFACT_DIR", "/tmp/cua-live-e2e"))
    summary = {
        "lane": lane,
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

    try:
        provisioning_attempted = True
        started = time.monotonic()
        async with Sandbox.ephemeral(
            Image.from_registry(IMAGE),
            name=namespace,
            cpu=4,
            memory_mb=4096,
            server_port=8000,
            time_to_start=900,
            telemetry_enabled=False,
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
                    assert (
                        pool_name == namespace
                    ), f"pool name {pool_name!r} must equal requested name {namespace!r}"

                template = await fleet.get_template(pool_name, pool_name)
                assert_template_contract(template, expected_port=8000)

                width, height = await sandbox.screen.size()
                summary["screen"] = {"width": width, "height": height}
                assert (width, height) == (1024, 768)

                screenshot = await sandbox.screenshot()
                artifact_dir.mkdir(parents=True, exist_ok=True)
                (artifact_dir / "screen.png").write_bytes(screenshot)
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

                if os.environ.get("CUA_LIVE_E2E_SIGNED_URLS") == "true":
                    signed_url = await sandbox.services.create_signed_url(
                        "server",
                        label="periodic-live-e2e",
                        expires_in_seconds=300,
                    )
                    assert signed_url.namespace == pool_name
                    assert signed_url.service == "server"
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
                try:
                    expected_inventory = {"templates": [], "pools": [], "claims": []}
                    inventory = await collect_resource_inventory(fleet, resource_namespace)
                    if claims_absent is True and primary_error is None:
                        inventory_deadline = time.monotonic() + 180.0
                        while (
                            inventory != expected_inventory
                            and time.monotonic() < inventory_deadline
                        ):
                            await asyncio.sleep(5.0)
                            inventory = await collect_resource_inventory(fleet, resource_namespace)
                    summary["persistent_resources"] = inventory
                except BaseException as error:
                    record_cleanup_error(error)

            if claims_absent is False:
                try:
                    summary["claim_leak"] = True
                    pytest.fail(f"claims remain in namespace {namespace} after Sandbox.ephemeral()")
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
    await run_fleet_ephemeral_live()
