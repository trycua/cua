from __future__ import annotations

from importlib.metadata import version
import os
from pathlib import Path
import re
import time

import pytest
import cua_sandbox
from cua_sandbox import Image, Sandbox

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_fleet_client,
    build_namespace_name,
    collect_resource_inventory,
    namespace_exists,
    wait_namespace_absent,
    write_summary,
)

IMAGE = (
    "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace-duo"
    "@sha256:5b9cb82f482834f7541901b87be956e7544d0db13fabc0b372cbc5eca5a74180"
)


def has_oauth_credentials() -> bool:
    return bool(os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET"))


def selected_namespace() -> str:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = os.environ.get("CUA_LIVE_E2E_NAMESPACE") or build_namespace_name(
        lane,
        os.environ.get("GITHUB_RUN_ID", str(int(time.time()))),
        os.environ.get("GITHUB_RUN_ATTEMPT", "1"),
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
        "source_sha": os.environ.get("GITHUB_SHA"),
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
        if await namespace_exists(fleet, namespace):
            raise RuntimeError(f"namespace {namespace} already exists")
        started = time.monotonic()
        async with Sandbox.ephemeral(
            Image.from_registry(IMAGE),
            name=namespace,
            cpu=4,
            memory_mb=4096,
            server_port=8000,
            time_to_start=900,
            request_timeout=60,
            telemetry_enabled=False,
        ) as sandbox:
            sandbox_yielded = True
            summary["provision_seconds"] = time.monotonic() - started
            summary["sandbox_name"] = sandbox.name
            try:
                template = await fleet.get_template(namespace, namespace)
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
        if sandbox_yielded:
            cleanup_started = time.monotonic()
            try:
                cleaned = await wait_namespace_absent(fleet, namespace)
                summary["automatic_cleanup"] = cleaned
                if not cleaned:
                    summary["namespace_leak"] = True
                    summary["remaining_resources"] = await collect_resource_inventory(
                        fleet, namespace
                    )
                    pytest.fail(f"namespace {namespace} leaked after Sandbox.ephemeral()")
                summary["cleanup_seconds"] = time.monotonic() - cleanup_started
            except BaseException as error:
                record_cleanup_error(error)
        else:
            summary["provisioning"] = {"sandbox_yielded": False}

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
