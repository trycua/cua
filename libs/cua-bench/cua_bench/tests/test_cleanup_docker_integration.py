"""Docker-backed integration: real daemon network -> production receipt -> verifier.

Requires a working Docker daemon. In CI, absence of Docker is a hard failure.
"""
import asyncio
import json
import os
import shutil
import uuid

import pytest

from cua_bench.runner import task_runner as module
from cua_bench.verify_execution import verify_execution


async def docker(*args):
    proc = await asyncio.create_subprocess_exec(
        "docker", *args, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await proc.communicate()
    if proc.returncode:
        raise AssertionError(f"docker {args[0]} failed: {stderr.decode(errors='replace')}")
    return stdout.decode().strip()


@pytest.mark.asyncio
async def test_real_docker_network_cleanup_receipt_and_offline_verification(tmp_path, monkeypatch):
    available = shutil.which("docker") is not None
    if not available:
        if os.environ.get("CI"):
            pytest.fail("CI must have a real Docker CLI and daemon for this acceptance test")
        pytest.skip("Local Docker not installed")
    await docker("info", "--format", "{{.ServerVersion}}")

    task_id = "integration-" + uuid.uuid4().hex[:12]
    network = "cua-task-" + task_id
    runner = module.TaskRunner()
    monkeypatch.setattr(module, "get_overlays_path", lambda: tmp_path / "overlays")
    await docker("network", "create", network)
    try:
        await runner._write_cleanup_receipt(task_id, str(tmp_path))
        first = json.loads((tmp_path / "cleanup-result.json").read_text())
        assert first["resources"]["network"]["absent"] is False
        assert first["verified"] is False
        assert verify_execution(tmp_path, 0, task_id)["checks"]["cleanup"] is False

        await docker("network", "rm", network)
        await runner._write_cleanup_receipt(task_id, str(tmp_path))
        second = json.loads((tmp_path / "cleanup-result.json").read_text())
        assert second["resources"]["network"]["absent"] is True
        assert second["verified"] is True
        assert verify_execution(tmp_path, 0, task_id)["checks"]["cleanup"] is True
    finally:
        # Never leave test-created Docker resources on the runner.
        proc = await asyncio.create_subprocess_exec(
            "docker", "network", "rm", network,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
        )
        await proc.wait()
