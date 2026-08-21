"""Live Apple Silicon Docker test for the local Linux sandbox path.

Run against an explicit candidate image:

    CUA_TEST_LOCAL_LINUX_DOCKER=1 \
    CUA_TEST_LINUX_DOCKER_IMAGE=public.ecr.aws/...:docker-main-<sha> \
      pytest tests/live/test_local_linux_docker.py -v

Omit CUA_TEST_LINUX_DOCKER_IMAGE to exercise the built-in Image.linux()
container resolution instead.
"""

from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import uuid
from pathlib import Path

import pytest
from cua_sandbox import Image, Sandbox


def _enabled() -> bool:
    return os.environ.get("CUA_TEST_LOCAL_LINUX_DOCKER", "").lower() in {
        "1",
        "true",
        "yes",
    }


def _docker_available() -> bool:
    if shutil.which("docker") is None:
        return False
    try:
        result = subprocess.run(
            ["docker", "info"],
            capture_output=True,
            timeout=15,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return result.returncode == 0


def _test_image() -> tuple[Image, str]:
    image_ref = os.environ.get("CUA_TEST_LINUX_DOCKER_IMAGE")
    if image_ref:
        return Image.from_registry(image_ref, os_type="linux", kind="container"), image_ref
    return Image.linux(kind="container"), "built-in Linux container image"


pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(not _enabled(), reason="CUA_TEST_LOCAL_LINUX_DOCKER is not enabled"),
    pytest.mark.skipif(not _docker_available(), reason="Docker is not available"),
]


async def test_local_linux_docker_on_apple_silicon() -> None:
    host_arch = platform.machine().lower()
    assert host_arch in {"arm64", "aarch64"}, f"Apple Silicon required, found {host_arch}"

    image, image_description = _test_image()
    name = f"cua-linux-arm64-live-{uuid.uuid4().hex[:8]}"
    clipboard_marker = f"{name}-{uuid.uuid4().hex}"
    artifact_dir = Path(os.environ.get("CUA_TEST_ARTIFACT_DIR", "/tmp/cua-linux-arm64-live"))

    summary = {
        "host_architecture": host_arch,
        "image": image_description,
        "sandbox_name": name,
    }

    async with Sandbox.ephemeral(
        image,
        local=True,
        name=name,
        telemetry_enabled=False,
    ) as sandbox:
        architecture = await sandbox.shell.run("uname -m")
        assert architecture.success, architecture.stderr
        summary["container_architecture"] = architecture.stdout.strip()
        assert architecture.stdout.strip() == "aarch64"

        screenshot = await sandbox.screenshot()
        assert screenshot.startswith(b"\x89PNG\r\n\x1a\n")
        assert len(screenshot) > 10_000

        width, height = await sandbox.get_dimensions()
        assert width > 0 and height > 0
        summary["screen"] = {"width": width, "height": height}

        await sandbox.clipboard.set(clipboard_marker)
        assert await sandbox.clipboard.get() == clipboard_marker

        artifact_dir.mkdir(parents=True, exist_ok=True)
        (artifact_dir / "screenshot.png").write_bytes(screenshot)
        (artifact_dir / "summary.json").write_text(
            json.dumps(summary, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    inspect = subprocess.run(
        ["docker", "inspect", "--type", "container", name],
        capture_output=True,
        check=False,
    )
    assert inspect.returncode != 0, f"ephemeral container {name!r} was not removed"
