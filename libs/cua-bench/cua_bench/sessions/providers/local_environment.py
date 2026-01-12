"""Local environment provider for managing sandbox containers.

This provider manages the lifecycle of sandbox containers (linux-docker,
windows-qemu, linux-qemu, android-qemu) for local task execution.

Features:
- QCOW2 overlay system for fast reset between tasks (QEMU environments)
- Worker allocation for parallel task execution
- Per-OS boot timeout configuration
- Session status and log retrieval

Usage:
    from cua_bench.sessions.providers.local_environment import LocalEnvironmentProvider

    provider = LocalEnvironmentProvider()
    env_info = await provider.start("linux-docker")
    # ... run tasks against env_info.api_url ...
    await provider.stop(env_info.env_id)
"""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import subprocess
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, Optional

if TYPE_CHECKING:
    import aiohttp


# =============================================================================
# Data Types
# =============================================================================


@dataclass
class EnvironmentInfo:
    """Information about a running sandbox environment."""

    env_id: str
    platform: str
    api_url: str
    vnc_url: str
    container_name: str
    api_port: int
    vnc_port: int
    image_name: str
    os_type: str  # "linux", "windows", "android"
    width: int = 1920
    height: int = 1080
    worker_id: Optional[int] = None  # Worker ID for overlay isolation


# =============================================================================
# Environment Configuration
# =============================================================================

# Container prefix for environments started by this provider
CONTAINER_PREFIX = "cua-bench-env-"

# Platform configurations
PLATFORM_CONFIGS = {
    "linux-docker": {
        "image": "trycua/cua-xfce:latest",
        "internal_vnc_port": 6901,
        "internal_api_port": 8000,
        "requires_kvm": False,
        "image_marker": None,
        "os_type": "linux",
        "boot_timeout": 60,  # Fast, no VM boot
        "use_overlays": False,  # Stateless container
    },
    "linux-qemu": {
        "image": "trycua/cua-qemu-linux:latest",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "image_marker": "linux.boot",
        "os_type": "linux",
        "boot_timeout": 120,  # QEMU VM boot
        "use_overlays": True,  # Enable QCOW2 overlays for fast reset
    },
    "windows-qemu": {
        "image": "trycua/cua-qemu-windows:latest",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "image_marker": "windows.boot",
        "os_type": "windows",
        "boot_timeout": 180,  # Windows is slow to boot
        "use_overlays": True,  # Enable QCOW2 overlays for fast reset
    },
    "android-qemu": {
        "image": "trycua/cua-qemu-android:latest",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "image_marker": "android.boot",
        "os_type": "android",
        "boot_timeout": 120,  # Android VM boot
        "use_overlays": True,  # Enable QCOW2 overlays for fast reset
    },
}


# =============================================================================
# Helper Functions
# =============================================================================


def get_data_dir() -> Path:
    """Get XDG data directory for cua-bench."""
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench"


def get_image_path(image_name: str) -> Path:
    """Get path to an image."""
    return get_data_dir() / "images" / image_name


def get_workers_path(platform: str) -> Path:
    """Get path to workers directory for a platform."""
    return get_data_dir() / "workers" / platform


def find_free_port(start: int = 5000, end: int = 9000) -> int:
    """Find a free port in the given range."""
    import socket

    for port in range(start, end):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.bind(("", port))
                return port
        except OSError:
            continue
    raise RuntimeError(f"No free port found in range {start}-{end}")


def check_docker() -> bool:
    """Check if Docker is running."""
    try:
        result = subprocess.run(["docker", "info"], capture_output=True, timeout=10)
        return result.returncode == 0
    except Exception:
        return False


def check_container_running(container_name: str) -> bool:
    """Check if a Docker container is running."""
    try:
        result = subprocess.run(
            ["docker", "ps", "-q", "-f", f"name=^{container_name}$"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return bool(result.stdout.strip())
    except Exception:
        return False


def check_image_exists(image_name: str) -> bool:
    """Check if a Docker image exists locally."""
    try:
        result = subprocess.run(
            ["docker", "images", "-q", image_name], capture_output=True, text=True, timeout=10
        )
        return bool(result.stdout.strip())
    except Exception:
        return False


def pull_image(image_name: str) -> None:
    """Pull Docker image from registry."""
    subprocess.run(["docker", "pull", image_name], check=True)


# =============================================================================
# Local Environment Provider
# =============================================================================


class LocalEnvironmentProvider:
    """Manages sandbox containers for local task execution.

    This provider handles:
    - Starting sandbox containers (linux-docker, windows-qemu, etc.)
    - Port allocation and management
    - Health checking
    - Container cleanup

    Example:
        provider = LocalEnvironmentProvider()

        # Start an environment
        env_info = await provider.start("windows-qemu", image_name="windows-waa")

        # Wait for it to be ready
        await provider.wait_until_ready(env_info.env_id, timeout=120)

        # Use the environment
        session = RemoteDesktopSession(api_url=env_info.api_url)
        screenshot = await session.screenshot()

        # Stop when done
        await provider.stop(env_info.env_id)
    """

    def __init__(self):
        """Initialize the provider."""
        self._running_envs: Dict[str, EnvironmentInfo] = {}
        self._aiohttp_session: Optional["aiohttp.ClientSession"] = None
        # Worker tracking for overlay isolation
        self._session_workers: Dict[str, int] = {}  # env_id -> worker_id

    async def _ensure_aiohttp_session(self) -> "aiohttp.ClientSession":
        """Ensure aiohttp session exists."""
        import aiohttp

        if self._aiohttp_session is None or self._aiohttp_session.closed:
            self._aiohttp_session = aiohttp.ClientSession()
        return self._aiohttp_session

    # =========================================================================
    # Worker Overlay Management
    # =========================================================================

    def _allocate_worker_id(self, env_type: str) -> int:
        """Allocate a unique worker ID for parallel execution.

        Args:
            env_type: Environment type (for path isolation)

        Returns:
            Unique worker ID not in use by active sessions
        """
        existing_workers = set()
        workers_path = get_workers_path(env_type)

        # Check existing worker directories
        if workers_path.exists():
            for d in workers_path.iterdir():
                if d.is_dir() and d.name.isdigit():
                    existing_workers.add(int(d.name))

        # Find first available worker ID not in use by active sessions
        worker_id = 0
        active_workers = set(self._session_workers.values())

        while worker_id in existing_workers or worker_id in active_workers:
            worker_id += 1

        return worker_id

    def _create_worker_overlay(self, env_type: str, worker_id: int) -> Path:
        """Create a CoW overlay directory for a worker.

        The QEMU container will use this directory for QCOW2 overlays,
        keeping the golden image pristine.

        Args:
            env_type: Environment type (for path isolation)
            worker_id: Worker ID to create overlay for

        Returns:
            Path to the worker overlay directory
        """
        worker_path = get_workers_path(env_type) / str(worker_id)

        # Clean up any existing overlay to ensure fresh state
        if worker_path.exists():
            shutil.rmtree(worker_path)

        worker_path.mkdir(parents=True, exist_ok=True)
        return worker_path

    def _cleanup_worker_overlay(self, env_type: str, worker_id: int) -> None:
        """Remove worker overlay directory.

        Args:
            env_type: Environment type (for path isolation)
            worker_id: Worker ID to clean up
        """
        worker_path = get_workers_path(env_type) / str(worker_id)
        if worker_path.exists():
            shutil.rmtree(worker_path)

    # =========================================================================
    # Environment Lifecycle
    # =========================================================================

    async def start(
        self,
        platform: str,
        image_name: Optional[str] = None,
        *,
        api_port: Optional[int] = None,
        vnc_port: Optional[int] = None,
        memory: str = "8G",
        cpus: str = "8",
        env_id: Optional[str] = None,
    ) -> EnvironmentInfo:
        """Start a sandbox container.

        Args:
            platform: Platform type (linux-docker, windows-qemu, etc.)
            image_name: Name of image to use (default: same as platform)
            api_port: API port to expose (default: auto-allocated)
            vnc_port: VNC port to expose (default: auto-allocated)
            memory: Memory allocation for QEMU VMs
            cpus: CPU cores for QEMU VMs
            env_id: Custom environment ID (default: auto-generated)

        Returns:
            EnvironmentInfo with connection details

        Raises:
            ValueError: If platform is not supported
            RuntimeError: If container fails to start
        """
        if platform not in PLATFORM_CONFIGS:
            raise ValueError(
                f"Unknown platform: {platform}. " f"Supported: {list(PLATFORM_CONFIGS.keys())}"
            )

        if not check_docker():
            raise RuntimeError("Docker is not running. Please start Docker and try again.")

        config = PLATFORM_CONFIGS[platform]
        image_name = image_name or platform

        # Validate image exists (for QEMU types)
        if config["image_marker"]:
            image_path = get_image_path(image_name)
            marker_path = image_path / config["image_marker"]
            if not marker_path.exists():
                raise RuntimeError(
                    f"Image '{image_name}' not found at {image_path}. "
                    f"Run: cb image create {platform}"
                )

        # Generate env_id and container name
        env_id = env_id or f"{platform}-{uuid.uuid4().hex[:8]}"
        container_name = f"{CONTAINER_PREFIX}{env_id}"

        # Allocate worker and create overlay for QEMU types
        worker_id: Optional[int] = None
        worker_path: Optional[Path] = None

        if config["use_overlays"]:
            worker_id = self._allocate_worker_id(platform)
            worker_path = self._create_worker_overlay(platform, worker_id)
            self._session_workers[env_id] = worker_id

        # Allocate ports
        if api_port is None:
            api_port = find_free_port(5000, 6000)
        if vnc_port is None:
            vnc_port = find_free_port(8000, 9000)

        # Pull Docker image if needed
        docker_image = config["image"]
        if not check_image_exists(docker_image):
            pull_image(docker_image)

        # Build docker command
        docker_cmd = self._build_docker_cmd(
            platform=platform,
            config=config,
            container_name=container_name,
            image_path=image_path,
            worker_path=worker_path,
            api_port=api_port,
            vnc_port=vnc_port,
            memory=memory,
            cpus=cpus,
        )

        # Start container
        result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            # Cleanup worker overlay on failure
            if worker_id is not None:
                self._cleanup_worker_overlay(platform, worker_id)
                self._session_workers.pop(env_id, None)
            raise RuntimeError(f"Failed to start container: {result.stderr}")

        # Create environment info
        env_info = EnvironmentInfo(
            env_id=env_id,
            platform=platform,
            api_url=f"http://localhost:{api_port}",
            vnc_url=f"http://localhost:{vnc_port}",
            container_name=container_name,
            api_port=api_port,
            vnc_port=vnc_port,
            image_name=image_name,
            os_type=config["os_type"],
            worker_id=worker_id,
        )

        self._running_envs[env_id] = env_info
        return env_info

    def _build_docker_cmd(
        self,
        platform: str,
        config: dict,
        container_name: str,
        image_path: Optional[Path],
        worker_path: Optional[Path],
        api_port: int,
        vnc_port: int,
        memory: str,
        cpus: str,
    ) -> list:
        """Build the docker run command.

        Args:
            platform: Platform type
            config: Platform configuration from PLATFORM_CONFIGS
            container_name: Name for the container
            image_path: Path to image (QEMU types only)
            worker_path: Path to worker overlay directory (QEMU types with overlays)
            api_port: Host port for API
            vnc_port: Host port for VNC
            memory: Memory allocation for QEMU VMs
            cpus: CPU cores for QEMU VMs

        Returns:
            Docker command as list of strings
        """
        docker_image = config["image"]
        internal_api = config["internal_api_port"]
        internal_vnc = config["internal_vnc_port"]

        if platform == "linux-docker":
            # Simple container, no QEMU
            return [
                "docker",
                "run",
                "-d",
                "--rm",
                "-p",
                f"{vnc_port}:{internal_vnc}",
                "-p",
                f"{api_port}:{internal_api}",
                "--name",
                container_name,
                docker_image,
            ]
        else:
            # QEMU-based container
            cmd = [
                "docker",
                "run",
                "-d",
                "-t",
                "--rm",
                "-p",
                f"{vnc_port}:{internal_vnc}",
                "-p",
                f"{api_port}:{internal_api}",
                "--name",
                container_name,
                "--platform",
                "linux/amd64",
                "--cap-add",
                "NET_ADMIN",
                "--stop-timeout",
                "120",
            ]

            # Mount storage: worker overlay (writable) over image (base)
            # The container's QEMU setup will create QCOW2 overlay on top of image
            if config["use_overlays"] and worker_path and image_path:
                # Mount image as read-only base, worker as writable overlay
                cmd.extend(["-v", f"{image_path}:/golden:ro"])
                cmd.extend(["-v", f"{worker_path}:/storage"])
            elif image_path:
                # No overlay - mount image directly (original behavior)
                cmd.extend(["-v", f"{image_path}:/storage"])

            # Add KVM support if available
            if os.path.exists("/dev/kvm"):
                cmd.extend(["--device=/dev/kvm"])
            else:
                cmd.extend(["-e", "KVM=N"])

            cmd.extend(["-e", f"RAM_SIZE={memory}"])
            cmd.extend(["-e", f"CPU_CORES={cpus}"])
            cmd.append(docker_image)
            return cmd

    async def stop(self, env_id: str, force: bool = False) -> None:
        """Stop and remove an environment container.

        Args:
            env_id: Environment ID to stop
            force: Force kill instead of graceful stop
        """
        env_info = self._running_envs.get(env_id)
        if not env_info:
            return

        container_name = env_info.container_name

        # Stop container
        if check_container_running(container_name):
            if force:
                subprocess.run(["docker", "kill", container_name], check=False)
            else:
                subprocess.run(["docker", "stop", container_name], check=False, timeout=150)

        # Clean up worker overlay if applicable
        worker_id = self._session_workers.get(env_id)
        if worker_id is not None:
            self._cleanup_worker_overlay(env_info.platform, worker_id)
            self._session_workers.pop(env_id, None)

        self._running_envs.pop(env_id, None)

    async def check_health(self, env_id: str) -> bool:
        """Check if an environment is healthy and ready.

        Args:
            env_id: Environment ID to check

        Returns:
            True if environment is healthy, False otherwise
        """
        env_info = self._running_envs.get(env_id)
        if not env_info:
            return False

        import aiohttp

        session = await self._ensure_aiohttp_session()

        try:
            async with session.get(
                f"{env_info.api_url}/status", timeout=aiohttp.ClientTimeout(total=5)
            ) as response:
                return response.status == 200
        except Exception:
            return False

    async def wait_until_ready(
        self,
        env_id: str,
        timeout: int = 120,
        poll_interval: float = 2.0,
    ) -> bool:
        """Wait until an environment is ready.

        Args:
            env_id: Environment ID to wait for
            timeout: Maximum time to wait in seconds
            poll_interval: Time between health checks

        Returns:
            True if environment became ready, False if timeout
        """
        import time

        start = time.time()

        while time.time() - start < timeout:
            if await self.check_health(env_id):
                return True
            await asyncio.sleep(poll_interval)

        return False

    async def get_info(self, env_id: str) -> Optional[EnvironmentInfo]:
        """Get information about a running environment.

        Args:
            env_id: Environment ID

        Returns:
            EnvironmentInfo if found, None otherwise
        """
        return self._running_envs.get(env_id)

    async def list_environments(self) -> Dict[str, EnvironmentInfo]:
        """List all environments managed by this provider.

        Returns:
            Dict mapping env_id to EnvironmentInfo
        """
        return dict(self._running_envs)

    # =========================================================================
    # Status and Logging
    # =========================================================================

    async def get_session_status(self, env_id: str) -> Dict[str, Any]:
        """Get detailed status of an environment session.

        Args:
            env_id: Environment ID to check

        Returns:
            Dict containing session status with keys:
            - status: "running", "stopped", "not_found", etc.
            - vm_ready: Whether VM API is responding (QEMU types)
            - api_port, vnc_port: Port numbers
        """
        env_info = self._running_envs.get(env_id)

        if not env_info:
            return {"status": "not_found", "env_id": env_id}

        container_name = env_info.container_name

        # Check Docker container status
        result = subprocess.run(
            ["docker", "inspect", container_name],
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode != 0:
            return {"status": "deleted", "env_id": env_id}

        try:
            inspect_data = json.loads(result.stdout)
            if not inspect_data:
                return {"status": "deleted", "env_id": env_id}

            state = inspect_data[0].get("State", {})

            if state.get("Running"):
                # Also check if VM is responsive (for QEMU types)
                vm_ready = await self.check_health(env_id)

                return {
                    "status": "running",
                    "env_id": env_id,
                    "vm_ready": vm_ready,
                    "api_port": env_info.api_port,
                    "vnc_port": env_info.vnc_port,
                    "worker_id": env_info.worker_id,
                    "platform": env_info.platform,
                    "os_type": env_info.os_type,
                }
            elif state.get("Status") == "exited":
                exit_code = state.get("ExitCode", -1)
                if exit_code == 0:
                    return {"status": "completed", "env_id": env_id}
                elif exit_code == 137:
                    return {"status": "stopped", "env_id": env_id}
                else:
                    return {"status": "failed", "env_id": env_id, "exit_code": exit_code}
            else:
                return {"status": "unknown", "env_id": env_id}

        except (json.JSONDecodeError, KeyError, IndexError) as e:
            return {"status": "error", "env_id": env_id, "error": str(e)}

    async def get_session_logs(self, env_id: str, tail: Optional[int] = None) -> str:
        """Get logs from an environment session.

        Args:
            env_id: Environment ID
            tail: Number of lines from end (None for all)

        Returns:
            Log output as string
        """
        env_info = self._running_envs.get(env_id)

        if not env_info:
            return f"Environment {env_id} not found"

        container_name = env_info.container_name

        docker_cmd = ["docker", "logs"]
        if tail is not None:
            docker_cmd.extend(["--tail", str(tail)])
        docker_cmd.append(container_name)

        result = subprocess.run(
            docker_cmd,
            capture_output=True,
            text=True,
            check=False,
        )

        logs = result.stdout
        if result.stderr:
            logs += "\n--- STDERR ---\n" + result.stderr

        return logs

    async def cleanup_all(self) -> None:
        """Stop all environments managed by this provider."""
        for env_id in list(self._running_envs.keys()):
            await self.stop(env_id, force=True)

        if self._aiohttp_session and not self._aiohttp_session.closed:
            await self._aiohttp_session.close()

    async def __aenter__(self):
        """Context manager entry."""
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit - cleanup all environments."""
        await self.cleanup_all()
