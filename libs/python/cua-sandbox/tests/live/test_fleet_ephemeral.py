from __future__ import annotations

from importlib.metadata import version
import os
from pathlib import Path
import time

import pytest
from cua_sandbox import Image, Sandbox

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_fleet_client,
    build_namespace_name,
    cleanup_namespace,
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
    return namespace


pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(not has_oauth_credentials(), reason="Fleet OAuth credentials not set"),
]


async def run_fleet_ephemeral_live() -> None:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = selected_namespace()
    artifact_dir = Path(
        os.environ.get("CUA_LIVE_E2E_ARTIFACT_DIR", "/tmp/cua-live-e2e")
    )
    summary = {
        "lane": lane,
        "namespace": namespace,
        "image": IMAGE,
        "source_sha": os.environ.get("GITHUB_SHA"),
        "packages": {
            "cua-sandbox": version("cua-sandbox"),
            "cua-fleet": version("cua-fleet"),
        },
    }
    fleet, http_client = build_fleet_client()
    primary_error: BaseException | None = None
    creation_attempted = False

    try:
        if await namespace_exists(fleet, namespace):
            raise RuntimeError(f"namespace {namespace} already exists")
        creation_attempted = True
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
            summary["provision_seconds"] = time.monotonic() - started
            summary["sandbox_name"] = sandbox.name
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
        raise
    finally:
        try:
            if creation_attempted:
                cleanup_started = time.monotonic()
                try:
                    cleaned = await wait_namespace_absent(fleet, namespace)
                    summary["automatic_cleanup"] = cleaned
                    if not cleaned:
                        summary["remaining_resources"] = await collect_resource_inventory(
                            fleet, namespace
                        )
                        summary["emergency_cleanup"] = await cleanup_namespace(namespace)
                        if primary_error is None:
                            pytest.fail(f"namespace {namespace} leaked after Sandbox.ephemeral()")
                    summary["cleanup_seconds"] = time.monotonic() - cleanup_started
                except BaseException as cleanup_error:
                    summary["cleanup_error"] = {"type": type(cleanup_error).__name__}
                    if primary_error is None:
                        raise
        finally:
            close_error: BaseException | None = None
            try:
                await http_client.aclose()
            except BaseException as error:
                close_error = error
                summary["close_error"] = {"type": type(error).__name__}
            try:
                write_summary(artifact_dir / "summary.json", summary)
            except BaseException as summary_error:
                summary["summary_error"] = {"type": type(summary_error).__name__}
                if primary_error is None and close_error is None:
                    raise
            if primary_error is None and close_error is not None:
                raise close_error


async def test_fleet_ephemeral_live() -> None:
    await run_fleet_ephemeral_live()
