"""Docker runtime — spins up containers with computer-server."""

from __future__ import annotations

import asyncio
import logging
import subprocess
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from cua_sandbox.image import Image

import httpx
from cua_sandbox.image import Image
from cua_sandbox.runtime.base import Runtime, RuntimeInfo
from cua_sandbox.runtime.images import (
    DEFAULT_API_PORT,
    DEFAULT_VNC_PORT,
    internal_ports,
    resolve_image,
)

logger = logging.getLogger(__name__)


def _find_free_port(start: int = 8000, end: int = 9000) -> int:
    """Return the first TCP port in [start, end) not currently bound."""
    import socket

    for port in range(start, end):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                s.bind(("", port))
                return port
            except OSError:
                continue
    raise RuntimeError(f"No free port found in range {start}–{end}")


def _docker_bin() -> str:
    """Return the resolved path to the docker CLI binary.

    Probes common install locations in addition to PATH — SSH sessions often
    have a stripped PATH that omits /usr/local/bin (e.g. OrbStack on macOS).
    Raises RuntimeError if docker is not found.
    """
    import os
    import shutil

    candidates = [
        "docker",
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/usr/bin/docker",
        os.path.expanduser("~/.docker/bin/docker"),
    ]
    for candidate in candidates:
        resolved = shutil.which(candidate) or candidate
        try:
            subprocess.run([resolved, "info"], capture_output=True, check=True, timeout=10)
            return resolved
        except (subprocess.SubprocessError, FileNotFoundError, OSError):
            continue
    raise RuntimeError(
        "Docker not found. Install from https://docker.com or ensure the docker CLI is on PATH."
    )


def _has_docker() -> bool:
    """Return True if a Docker daemon is reachable."""
    try:
        _docker_bin()
        return True
    except RuntimeError:
        return False


def _has_kvm() -> bool:
    """Check if /dev/kvm is available (Linux/WSL2 only)."""
    import platform

    if platform.system() != "Linux":
        return False
    from pathlib import Path

    return Path("/dev/kvm").exists()


class DockerRuntime(Runtime):
    """Runs containers via ``docker run``."""

    def __init__(
        self,
        *,
        api_port: int = DEFAULT_API_PORT,
        vnc_port: int = DEFAULT_VNC_PORT,
        ephemeral: bool = True,
        volumes: Optional[list[str]] = None,
        environment: Optional[dict[str, str]] = None,
        devices: Optional[list[str]] = None,
        platform: Optional[str] = None,
        privileged: bool = False,
        stop_timeout: int = 120,
    ):
        self.api_port = api_port
        self.vnc_port = vnc_port
        self.ephemeral = ephemeral
        self.volumes = volumes or []
        self.environment = environment or {}
        self.devices = devices or []
        self.platform = platform
        self.privileged = privileged
        self.stop_timeout = stop_timeout

    async def start(self, image: Image, name: str, **opts) -> RuntimeInfo:
        if not _has_docker():
            raise RuntimeError("Docker is not installed or not running")

        docker_image = resolve_image(image.os_type, image._registry)
        internal_api, internal_vnc = internal_ports(docker_image)
        extra_flags: list[str] = []

        if "qemu" in docker_image:
            extra_flags += ["--cap-add", "NET_ADMIN"]

        # Volume mounts
        for v in self.volumes:
            extra_flags += ["-v", v]

        # Environment variables (from DockerRuntime constructor)
        for k, val in self.environment.items():
            extra_flags += ["-e", f"{k}={val}"]

        # Environment variables from image.env() calls
        for k, val in getattr(image, "_env", ()):
            extra_flags += ["-e", f"{k}={val}"]

        # Devices (e.g. /dev/kvm)
        for d in self.devices:
            extra_flags += ["--device", d]

        # Platform (e.g. linux/amd64)
        if self.platform:
            extra_flags += ["--platform", self.platform]

        # Privileged mode
        if self.privileged:
            extra_flags.append("--privileged")

        # Stop timeout
        extra_flags += ["--stop-timeout", str(self.stop_timeout)]

        # Expose additional ports from image._ports
        for port in getattr(image, "_ports", ()):
            host_p = _find_free_port(port, port + 1000)
            extra_flags += ["-p", f"{host_p}:{port}"]

        # Remove existing container with same name
        docker = _docker_bin()
        subprocess.run([docker, "rm", "-f", name], capture_output=True)

        api_port = _find_free_port(self.api_port)
        vnc_port = _find_free_port(api_port + 1)

        cmd = [
            docker,
            "run",
            "-d",
            "--name",
            name,
            "--label",
            "cua.sandbox=true",
            "--label",
            f"cua.sandbox.name={name}",
            "-p",
            f"{api_port}:{internal_api}",
            "-p",
            f"{vnc_port}:{internal_vnc}",
            *extra_flags,
            docker_image,
        ]
        logger.info(f"Starting container: {' '.join(cmd)}")
        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(f"docker run failed: {result.stderr}")

        container_id = result.stdout.strip()[:12]
        info = RuntimeInfo(
            host="localhost",
            api_port=api_port,
            vnc_port=vnc_port,
            container_id=container_id,
            name=name,
        )
        await self.is_ready(info)

        # Apply image layers and files via computer-server.
        # If any provisioning step fails, stop and remove the half-baked container
        # so callers don't get a broken sandbox silently left running.
        env_items = getattr(image, "_env", ())
        file_items = getattr(image, "_files", ())
        has_work = image._layers or file_items
        if has_work or env_items:
            try:
                from cua_sandbox.builder.executor import LayerExecutor

                executor = LayerExecutor(
                    f"http://{info.host}:{info.api_port}", os_type=image.os_type
                )

                # Write env vars to a sourceable profile script so run layers can access them
                if env_items and image.os_type != "windows":
                    import re as _re
                    import shlex as _shlex

                    # Validate all keys up front
                    for k, _ in env_items:
                        if not _re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                            raise ValueError(f"Unsafe env var name: {k!r}")
                    await executor.run_command(
                        "printf '#!/bin/sh\\n' | sudo tee /etc/profile.d/cua-env.sh > /dev/null"
                    )
                    for k, v in env_items:
                        quoted_v = _shlex.quote(v)
                        await executor.run_command(
                            f"printf 'export {k}=%s\\n' {quoted_v} "
                            f"| sudo tee -a /etc/profile.d/cua-env.sh > /dev/null"
                        )

                # Apply files before layers so later run layers can reference copied files
                for src, dst in file_items:
                    await executor.execute_layers([{"type": "copy", "src": src, "dst": dst}])

                if image._layers:
                    await executor.execute_layers(list(image._layers))
            except Exception:
                # Tear down the container so it doesn't linger in a broken state
                try:
                    docker = _docker_bin()
                    subprocess.run([docker, "rm", "-f", name], capture_output=True)
                except Exception:
                    pass
                raise

        return info

    async def stop(self, name: str) -> None:
        docker = _docker_bin()
        subprocess.run([docker, "stop", name], capture_output=True)
        if self.ephemeral:
            subprocess.run([docker, "rm", name], capture_output=True)

    async def suspend(self, name: str) -> None:
        """Pause a running Docker container."""
        subprocess.run([_docker_bin(), "pause", name], capture_output=True)

    async def resume(self, image: "Image", name: str, **opts) -> RuntimeInfo:
        """Unpause a paused Docker container and return its RuntimeInfo."""
        docker = _docker_bin()
        subprocess.run([docker, "unpause", name], capture_output=True)
        # Inspect to get mapped ports
        result = subprocess.run(
            [
                docker,
                "inspect",
                "--format",
                '{{(index (index .NetworkSettings.Ports "8000/tcp") 0).HostPort}}',
                name,
            ],
            capture_output=True,
            text=True,
        )
        api_port = int(result.stdout.strip()) if result.stdout.strip().isdigit() else self.api_port
        result2 = subprocess.run(
            [
                docker,
                "inspect",
                "--format",
                '{{(index (index .NetworkSettings.Ports "5900/tcp") 0).HostPort}}',
                name,
            ],
            capture_output=True,
            text=True,
        )
        vnc_port = (
            int(result2.stdout.strip()) if result2.stdout.strip().isdigit() else self.vnc_port
        )
        info = RuntimeInfo(host="localhost", api_port=api_port, vnc_port=vnc_port, name=name)
        await self.is_ready(info)
        return info

    async def list(self) -> list[dict]:
        """List Docker containers with the cua.sandbox=true label."""
        result = subprocess.run(
            [
                _docker_bin(),
                "ps",
                "-a",
                "--filter",
                "label=cua.sandbox=true",
                "--format",
                "{{.Names}}\t{{.Status}}",
            ],
            capture_output=True,
            text=True,
        )
        rows = []
        for line in result.stdout.strip().splitlines():
            if not line:
                continue
            parts = line.split("\t", 1)
            name_col = parts[0].strip()
            raw_status = parts[1].strip() if len(parts) > 1 else ""
            if raw_status.startswith("Up"):
                status = "running"
            elif raw_status.startswith("Paused") or "(Paused)" in raw_status:
                status = "suspended"
            elif raw_status.startswith("Exited"):
                status = "stopped"
            else:
                status = raw_status.lower()
            rows.append({"name": name_col, "status": status, "runtime_type": "docker"})
        return rows

    async def is_ready(self, info: RuntimeInfo, timeout: float = 120) -> bool:
        url = f"http://{info.host}:{info.api_port}/status"
        deadline = asyncio.get_event_loop().time() + timeout
        async with httpx.AsyncClient(timeout=5) as client:
            while asyncio.get_event_loop().time() < deadline:
                try:
                    resp = await client.get(url)
                    if resp.status_code == 200:
                        logger.info(f"Container {info.name} is ready")
                        return True
                except (
                    httpx.ConnectError,
                    httpx.ReadTimeout,
                    httpx.RemoteProtocolError,
                    httpx.ConnectTimeout,
                    httpx.ReadError,
                ):
                    pass
                await asyncio.sleep(2)
        raise TimeoutError(f"Container {info.name} not ready after {timeout}s")
