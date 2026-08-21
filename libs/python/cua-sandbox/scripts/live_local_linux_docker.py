#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cua-sandbox"]
#
# [tool.uv.sources]
# cua-sandbox = { path = "..", editable = true }
#
# [[tool.uv.index]]
# name = "cua-wheels"
# url = "https://wheels.cua.ai/simple"
# ///
"""Exercise the local Linux Docker sandbox on a real host."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import platform
import subprocess
import uuid
from pathlib import Path
from typing import Any

from cua_sandbox import Image, Sandbox


def run_command(*command: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, capture_output=True, text=True, check=False)


def require_docker() -> str:
    result = run_command("docker", "version", "--format", "{{.Server.Version}}")
    if result.returncode != 0:
        raise RuntimeError(f"Docker is unavailable: {result.stderr.strip()}")
    return result.stdout.strip()


def expected_container_architecture(host_architecture: str) -> str:
    normalized = host_architecture.lower()
    if normalized in {"arm64", "aarch64"}:
        return "aarch64"
    if normalized in {"amd64", "x86_64"}:
        return "x86_64"
    raise RuntimeError(f"Unsupported host architecture: {host_architecture}")


def write_summary(artifact_dir: Path, summary: dict[str, Any]) -> None:
    artifact_dir.mkdir(parents=True, exist_ok=True)
    (artifact_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def collect_container_diagnostics(name: str, artifact_dir: Path) -> None:
    artifact_dir.mkdir(parents=True, exist_ok=True)
    for label, command in {
        "docker-inspect.json": ("docker", "inspect", name),
        "docker.log": ("docker", "logs", name),
    }.items():
        result = run_command(*command)
        output = result.stdout
        if result.stderr:
            output += f"\n--- stderr ---\n{result.stderr}"
        (artifact_dir / label).write_text(output, encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the cua-sandbox local Linux Docker live test.",
    )
    parser.add_argument(
        "--image",
        default=os.environ.get("CUA_TEST_LINUX_DOCKER_IMAGE"),
        help="Candidate image reference. Omit to test the built-in Linux container image.",
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=Path(os.environ.get("CUA_TEST_ARTIFACT_DIR", "/tmp/cua-linux-arm64-live")),
        help="Directory for screenshot, summary, and failure diagnostics.",
    )
    parser.add_argument(
        "--allow-non-arm64",
        action="store_true",
        help="Permit execution on amd64 for diagnostic use.",
    )
    return parser.parse_args()


async def run_live_test(args: argparse.Namespace) -> None:
    host_architecture = platform.machine().lower()
    if not args.allow_non_arm64 and host_architecture not in {"arm64", "aarch64"}:
        raise RuntimeError(
            f"Apple Silicon is required, found {host_architecture}. "
            "Pass --allow-non-arm64 only for diagnostic runs."
        )

    docker_version = require_docker()
    expected_architecture = expected_container_architecture(host_architecture)
    if args.image:
        image = Image.from_registry(args.image, os_type="linux", kind="container")
        image_description = args.image
    else:
        image = Image.linux(kind="container")
        image_description = "built-in Linux container image"

    name = f"cua-linux-live-{uuid.uuid4().hex[:8]}"
    clipboard_marker = f"{name}-{uuid.uuid4().hex}"
    summary: dict[str, Any] = {
        "docker_server_version": docker_version,
        "host_architecture": host_architecture,
        "image": image_description,
        "sandbox_name": name,
        "success": False,
    }

    print(f"Host architecture: {host_architecture}")
    print(f"Docker server: {docker_version}")
    print(f"Image: {image_description}")
    print(f"Artifacts: {args.artifact_dir}")

    try:
        async with Sandbox.ephemeral(
            image,
            local=True,
            name=name,
            telemetry_enabled=False,
        ) as sandbox:
            try:
                architecture = await sandbox.shell.run("uname -m")
                if not architecture.success:
                    raise AssertionError(architecture.stderr)
                container_architecture = architecture.stdout.strip()
                summary["container_architecture"] = container_architecture
                if container_architecture != expected_architecture:
                    raise AssertionError(
                        f"expected container architecture {expected_architecture}, "
                        f"got {container_architecture}"
                    )

                screenshot = await sandbox.screenshot()
                if not screenshot.startswith(b"\x89PNG\r\n\x1a\n"):
                    raise AssertionError("screenshot is not a PNG")
                if len(screenshot) <= 10_000:
                    raise AssertionError(f"screenshot is suspiciously small: {len(screenshot)} bytes")
                args.artifact_dir.mkdir(parents=True, exist_ok=True)
                (args.artifact_dir / "screenshot.png").write_bytes(screenshot)
                summary["screenshot_bytes"] = len(screenshot)

                width, height = await sandbox.get_dimensions()
                if width <= 0 or height <= 0:
                    raise AssertionError(f"invalid screen dimensions: {width}x{height}")
                summary["screen"] = {"width": width, "height": height}

                await sandbox.clipboard.set(clipboard_marker)
                clipboard_value = await sandbox.clipboard.get()
                if clipboard_value != clipboard_marker:
                    raise AssertionError(
                        f"clipboard mismatch: expected {clipboard_marker!r}, got {clipboard_value!r}"
                    )
                summary["clipboard_round_trip"] = True
            except BaseException:
                collect_container_diagnostics(name, args.artifact_dir)
                raise
    except BaseException as error:
        summary["error"] = {"message": str(error), "type": type(error).__name__}
        write_summary(args.artifact_dir, summary)
        raise

    inspect = run_command("docker", "inspect", "--type", "container", name)
    if inspect.returncode == 0:
        summary["cleanup_error"] = f"ephemeral container {name!r} was not removed"
        write_summary(args.artifact_dir, summary)
        raise AssertionError(summary["cleanup_error"])

    summary["cleanup_verified"] = True
    summary["success"] = True
    write_summary(args.artifact_dir, summary)
    print("PASS: local Linux Docker sandbox is healthy")


def main() -> None:
    asyncio.run(run_live_test(parse_args()))


if __name__ == "__main__":
    main()
