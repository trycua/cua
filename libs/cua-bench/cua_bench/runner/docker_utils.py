"""Docker utilities for 2-container task execution."""

import asyncio
import platform
import shutil
import subprocess
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional


def create_overlay_copy(golden_path: Path, overlay_path: Path, verbose: bool = False) -> None:
    """Copy golden image to overlay directory for COW-like behavior.

    This is a temporary workaround until proper QEMU overlay support is added.
    Uses native `cp -a` on Unix for speed (5x faster than Python shutil).
    Falls back to shutil.copytree on Windows.

    WIP: https://github.com/trycua/cua/issues/699

    Args:
        golden_path: Path to golden image directory
        overlay_path: Path to overlay directory (will be created/cleaned)
        verbose: Print progress messages

    Raises:
        RuntimeError: If copy fails
    """
    # Clean up any existing overlay
    if overlay_path.exists():
        shutil.rmtree(overlay_path)
    overlay_path.mkdir(parents=True, exist_ok=True)

    if verbose:
        print(f"   Source:  {golden_path}")
        print(f"   Overlay: {overlay_path}")
        print("   (This may take a while for large images)")
        print("   WIP: https://github.com/trycua/cua/issues/699")

    # Use platform-specific copy method
    if platform.system() == "Windows":
        # On Windows, use shutil.copytree (robocopy would be faster but more complex)
        try:
            # copytree expects dest to not exist, but we need to copy contents into it
            for item in golden_path.iterdir():
                src = golden_path / item.name
                dst = overlay_path / item.name
                if src.is_dir():
                    shutil.copytree(src, dst)
                else:
                    shutil.copy2(src, dst)
        except Exception as e:
            raise RuntimeError(f"Failed to create overlay: {e}")
    else:
        # On Unix, use native cp -a for speed (5x faster than Python shutil)
        result = subprocess.run(
            ["cp", "-a", f"{golden_path}/.", str(overlay_path)], capture_output=True, text=True
        )
        if result.returncode != 0:
            raise RuntimeError(f"Failed to create overlay: {result.stderr or 'cp failed'}")


def find_free_port(start: int = 5000, end: int = 9000) -> int:
    """Find a free port in the given range.

    Args:
        start: Start of port range (inclusive)
        end: End of port range (exclusive)

    Returns:
        First available port in the range

    Raises:
        RuntimeError: If no free port found
    """
    import socket

    for port in range(start, end):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.bind(("", port))
                return port
        except OSError:
            continue
    raise RuntimeError(f"No free port found in range {start}-{end}")


def allocate_ports(vnc_default: int = 8006, api_default: int = 5000) -> tuple[int, int]:
    """Allocate VNC and API ports, auto-selecting if defaults are in use.

    Args:
        vnc_default: Preferred VNC port
        api_default: Preferred API port

    Returns:
        Tuple of (vnc_port, api_port)
    """
    import socket

    def is_port_free(port: int) -> bool:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.bind(("", port))
                return True
        except OSError:
            return False

    # Try default ports first
    vnc_port = vnc_default if is_port_free(vnc_default) else find_free_port(8000, 9000)
    api_port = api_default if is_port_free(api_default) else find_free_port(5000, 6000)

    return vnc_port, api_port


@dataclass
class ContainerInfo:
    """Information about a running container."""

    container_id: str
    container_name: str
    network: str
    hostname: str


async def create_network(name: str) -> str:
    """Create a Docker network for task isolation.

    Args:
        name: Network name

    Returns:
        Network name (same as input)
    """
    process = await asyncio.create_subprocess_exec(
        "docker",
        "network",
        "create",
        name,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await process.communicate()

    if process.returncode != 0:
        # Check if network already exists
        if b"already exists" in stderr:
            return name
        raise RuntimeError(f"Failed to create network {name}: {stderr.decode()}")

    return name


async def remove_network(name: str) -> None:
    """Remove a Docker network.

    Args:
        name: Network name to remove
    """
    process = await asyncio.create_subprocess_exec(
        "docker",
        "network",
        "rm",
        name,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    await process.communicate()
    # Ignore errors - network may already be gone


async def start_container(
    image: str,
    name: str,
    network: str,
    hostname: str,
    *,
    env_vars: Optional[Dict[str, str]] = None,
    volumes: Optional[List[str]] = None,
    ports: Optional[Dict[int, int]] = None,
    command: Optional[List[str]] = None,
    detach: bool = True,
    privileged: bool = False,
    devices: Optional[List[str]] = None,
    remove_on_exit: bool = True,
    stop_timeout: int = 120,
    platform: Optional[str] = None,
    shm_size: Optional[str] = None,
    cap_add: Optional[List[str]] = None,
) -> ContainerInfo:
    """Start a container attached to a network.

    Args:
        image: Docker image name
        name: Container name
        network: Docker network name
        hostname: Container hostname within network
        env_vars: Environment variables to set
        volumes: Volume mounts (format: "host_path:container_path[:ro]")
        ports: Port mappings {host_port: container_port} (optional, for debugging)
        command: Command to run in container
        detach: Run in detached mode
        privileged: Run with --privileged
        devices: Devices to add (e.g., "/dev/kvm")
        remove_on_exit: Remove container when it exits
        stop_timeout: Timeout in seconds for graceful stop
        platform: Platform (e.g., "linux/amd64")
        shm_size: Shared memory size (e.g., "16g")
        cap_add: Capabilities to add (e.g., "NET_ADMIN")

    Returns:
        ContainerInfo with container details
    """
    cmd = ["docker", "run"]

    if detach:
        cmd.append("-d")

    if remove_on_exit:
        cmd.append("--rm")

    # Network and hostname
    cmd.extend(["--network", network])
    cmd.extend(["--hostname", hostname])
    cmd.extend(["--name", name])

    # Optional flags
    if privileged:
        cmd.append("--privileged")

    if platform:
        cmd.extend(["--platform", platform])

    if shm_size:
        cmd.extend(["--shm-size", shm_size])

    if stop_timeout:
        cmd.extend(["--stop-timeout", str(stop_timeout)])

    # Devices
    if devices:
        for device in devices:
            cmd.extend(["--device", device])

    # Capabilities
    if cap_add:
        for cap in cap_add:
            cmd.extend(["--cap-add", cap])

    # Environment variables
    if env_vars:
        for key, value in env_vars.items():
            cmd.extend(["-e", f"{key}={value}"])

    # Volumes
    if volumes:
        for volume in volumes:
            cmd.extend(["-v", volume])

    # Port mappings (optional, for debugging)
    if ports:
        for host_port, container_port in ports.items():
            cmd.extend(["-p", f"{host_port}:{container_port}"])

    # Image
    cmd.append(image)

    # Command
    if command:
        cmd.extend(command)

    # Execute
    process = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await process.communicate()

    if process.returncode != 0:
        raise RuntimeError(f"Failed to start container {name}: {stderr.decode()}")

    container_id = stdout.decode().strip()

    return ContainerInfo(
        container_id=container_id,
        container_name=name,
        network=network,
        hostname=hostname,
    )


async def stop_container(name: str, force: bool = False) -> None:
    """Stop a container.

    Args:
        name: Container name
        force: Use docker kill instead of docker stop
    """
    cmd = ["docker", "kill" if force else "stop", name]

    process = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    await process.communicate()
    # Ignore errors - container may already be stopped


async def remove_container(name: str) -> None:
    """Remove a container.

    Args:
        name: Container name
    """
    process = await asyncio.create_subprocess_exec(
        "docker",
        "rm",
        "-f",
        name,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    await process.communicate()
    # Ignore errors - container may already be gone


async def wait_for_container(name: str, timeout: Optional[int] = None) -> int:
    """Wait for a container to exit.

    Args:
        name: Container name
        timeout: Timeout in seconds (None = no timeout)

    Returns:
        Exit code of the container
    """
    cmd = ["docker", "wait", name]

    try:
        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        if timeout:
            stdout, stderr = await asyncio.wait_for(
                process.communicate(),
                timeout=timeout,
            )
        else:
            stdout, stderr = await process.communicate()

        if process.returncode != 0:
            raise RuntimeError(f"Failed to wait for container {name}: {stderr.decode()}")

        return int(stdout.decode().strip())

    except asyncio.TimeoutError:
        # Container didn't exit within timeout - kill it
        await stop_container(name, force=True)
        raise


async def get_container_logs(name: str, tail: Optional[int] = None) -> str:
    """Get container logs.

    Args:
        name: Container name
        tail: Number of lines from end (None = all)

    Returns:
        Container logs as string
    """
    cmd = ["docker", "logs"]

    if tail:
        cmd.extend(["--tail", str(tail)])

    cmd.append(name)

    process = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    stdout, _ = await process.communicate()

    return stdout.decode()


async def stream_container_logs_to_file(name: str, output_file: Path) -> asyncio.subprocess.Process:
    """Stream container logs to a file in real-time.

    Starts `docker logs -f` and writes output to the specified file.
    Returns the process handle so it can be cancelled later.

    Args:
        name: Container name
        output_file: Path to output log file

    Returns:
        The subprocess.Process handle (call process.terminate() to stop)
    """
    # Create parent directory if needed
    output_file.parent.mkdir(parents=True, exist_ok=True)

    # Open log file for writing
    log_handle = open(output_file, "w", encoding="utf-8", buffering=1)

    # Start docker logs -f process
    process = await asyncio.create_subprocess_exec(
        "docker",
        "logs",
        "-f",
        name,
        stdout=log_handle,
        stderr=asyncio.subprocess.STDOUT,
    )

    return process


async def container_is_running(name: str) -> bool:
    """Check if a container is running.

    Args:
        name: Container name

    Returns:
        True if container is running
    """
    process = await asyncio.create_subprocess_exec(
        "docker",
        "inspect",
        "-f",
        "{{.State.Running}}",
        name,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, _ = await process.communicate()

    return stdout.decode().strip().lower() == "true"


def generate_task_id() -> str:
    """Generate a unique task ID.

    Returns:
        Short unique identifier
    """
    return uuid.uuid4().hex[:8]


async def cleanup_stale_containers(prefix: str = "cua-") -> int:
    """Find and remove any stale containers with the given prefix.

    This cleans up containers that may have been left behind from
    previous runs (e.g., due to crashes or interruptions).

    Args:
        prefix: Container name prefix to match (default: "cua-")

    Returns:
        Number of containers removed
    """
    # List all containers (including stopped) with the prefix
    process = await asyncio.create_subprocess_exec(
        "docker",
        "ps",
        "-a",
        "--filter",
        f"name={prefix}",
        "--format",
        "{{.Names}}",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, _ = await process.communicate()

    containers = [c.strip() for c in stdout.decode().split("\n") if c.strip()]

    if not containers:
        return 0

    # Force remove all matching containers
    for name in containers:
        await stop_container(name, force=True)
        await remove_container(name)

    return len(containers)


async def cleanup_stale_networks(prefix: str = "cua-task-") -> int:
    """Find and remove any stale networks with the given prefix.

    Args:
        prefix: Network name prefix to match (default: "cua-task-")

    Returns:
        Number of networks removed
    """
    process = await asyncio.create_subprocess_exec(
        "docker",
        "network",
        "ls",
        "--filter",
        f"name={prefix}",
        "--format",
        "{{.Name}}",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, _ = await process.communicate()

    networks = [n.strip() for n in stdout.decode().split("\n") if n.strip()]

    if not networks:
        return 0

    for name in networks:
        await remove_network(name)

    return len(networks)


async def remove_image(image: str) -> bool:
    """Remove a Docker image.

    Args:
        image: Image name or ID to remove

    Returns:
        True if image was removed, False if not found or error
    """
    process = await asyncio.create_subprocess_exec(
        "docker",
        "rmi",
        "-f",
        image,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    _, _ = await process.communicate()
    return process.returncode == 0


async def prune_unused_images(filter_label: Optional[str] = None) -> int:
    """Remove unused Docker images.

    Args:
        filter_label: Optional label filter (e.g., "cua-bench=true")

    Returns:
        Number of images removed
    """
    cmd = ["docker", "image", "prune", "-f"]
    if filter_label:
        cmd.extend(["--filter", f"label={filter_label}"])

    process = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, _ = await process.communicate()

    # Parse output to count removed images
    output = stdout.decode()
    # Count lines with "deleted:" in them
    return output.count("deleted:")


async def full_cleanup() -> dict:
    """Perform a full cleanup of all cua-bench containers and networks.

    This is useful for cleaning up after crashes or before running new tasks.

    Returns:
        Dict with counts: {"containers": N, "networks": N}
    """
    containers = await cleanup_stale_containers("cua-")
    networks = await cleanup_stale_networks("cua-task-")

    return {
        "containers": containers,
        "networks": networks,
    }
