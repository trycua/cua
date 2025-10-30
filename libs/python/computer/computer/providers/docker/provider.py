"""
Docker VM provider implementation.

This provider uses Docker containers running the CUA Ubuntu image to create
Linux VMs with computer-server. It handles VM lifecycle operations through Docker
commands and container management.
"""

import asyncio
import json
import logging
import re
import subprocess
import time
from typing import Any, Dict, List, Optional

from ..base import BaseVMProvider, VMProviderType

# Setup logging
logger = logging.getLogger(__name__)

# Check if Docker is available
try:
    subprocess.run(["docker", "--version"], capture_output=True, check=True)
    HAS_DOCKER = True
except (subprocess.SubprocessError, FileNotFoundError):
    HAS_DOCKER = False


class DockerProvider(BaseVMProvider):
    """
    Docker VM Provider implementation using Docker containers.

    This provider uses Docker to run containers with the CUA Ubuntu image
    that includes computer-server for remote computer use.
    """

    def __init__(
        self,
        port: Optional[int] = 8000,
        host: str = "localhost",
        storage: Optional[str] = None,
        shared_path: Optional[str] = None,
        image: str = "trycua/cua-ubuntu:latest",
        verbose: bool = False,
        ephemeral: bool = False,
        vnc_port: Optional[int] = 6901,
    ):
        """Initialize the Docker VM Provider.

        Args:
            port: Currently unused (VM provider port)
            host: Hostname for the API server (default: localhost)
            storage: Path for persistent VM storage
            shared_path: Path for shared folder between host and container
            image: Docker image to use (default: "trycua/cua-ubuntu:latest")
                   Supported images:
                   - "trycua/cua-ubuntu:latest" (Kasm-based)
                   - "trycua/cua-xfce:latest" (vanilla XFCE)
            verbose: Enable verbose logging
            ephemeral: Use ephemeral (temporary) storage
            vnc_port: Port for VNC interface (default: 6901)
        """
        self.host = host
        self.api_port = 8000
        self.vnc_port = vnc_port
        self.ephemeral = ephemeral

        # Handle ephemeral storage (temporary directory)
        if ephemeral:
            self.storage = "ephemeral"
        else:
            self.storage = storage

        self.shared_path = shared_path
        self.image = image
        self.verbose = verbose
        self._container_id = None
        self._running_containers = {}  # Track running containers by name

        # Detect image type and configure user directory accordingly
        self._detect_image_config()

    def _detect_image_config(self):
        """Detect image type and configure paths accordingly."""
        # Detect if this is a docker-xfce image or Kasm image
        if "docker-xfce" in self.image.lower() or "xfce" in self.image.lower():
            self._home_dir = "/home/cua"
            self._image_type = "docker-xfce"
            logger.info(f"Detected docker-xfce image: using {self._home_dir}")
        else:
            # Default to Kasm configuration
            self._home_dir = "/home/kasm-user"
            self._image_type = "kasm"
            logger.info(f"Detected Kasm image: using {self._home_dir}")

    @property
    def provider_type(self) -> VMProviderType:
        """Return the provider type."""
        return VMProviderType.DOCKER

    def _parse_memory(self, memory_str: str) -> str:
        """Parse memory string to Docker format.

        Examples:
            "8GB" -> "8g"
            "1024MB" -> "1024m"
            "512" -> "512m"
        """
        if isinstance(memory_str, int):
            return f"{memory_str}m"

        if isinstance(memory_str, str):
            # Extract number and unit
            match = re.match(r"(\d+)([A-Za-z]*)", memory_str)
            if match:
                value, unit = match.groups()
                unit = unit.upper()

                if unit == "GB" or unit == "G":
                    return f"{value}g"
                elif unit == "MB" or unit == "M" or unit == "":
                    return f"{value}m"

        # Default fallback
        logger.warning(f"Could not parse memory string '{memory_str}', using 4g default")
        return "4g"  # Default to 4GB

    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Get VM information by name.

        Args:
            name: Name of the VM to get information for
            storage: Optional storage path override. If provided, this will be used
                    instead of the provider's default storage path.

        Returns:
            Dictionary with VM information including status, IP address, etc.
        """
        try:
            # Check if container exists and get its status
            cmd = ["docker", "inspect", name]
            result = subprocess.run(cmd, capture_output=True, text=True)

            if result.returncode != 0:
                # Container doesn't exist
                return {
                    "name": name,
                    "status": "not_found",
                    "ip_address": None,
                    "ports": {},
                    "image": self.image,
                    "provider": "docker",
                }

            # Parse container info
            container_info = json.loads(result.stdout)[0]
            state = container_info["State"]
            network_settings = container_info["NetworkSettings"]

            # Determine status
            if state["Running"]:
                status = "running"
            elif state["Paused"]:
                status = "paused"
            else:
                status = "stopped"

            # Get IP address
            ip_address = network_settings.get("IPAddress", "")
            if not ip_address and "Networks" in network_settings:
                # Try to get IP from bridge network
                for network_name, network_info in network_settings["Networks"].items():
                    if network_info.get("IPAddress"):
                        ip_address = network_info["IPAddress"]
                        break

            # Get port mappings
            ports = {}
            if "Ports" in network_settings and network_settings["Ports"]:
                # network_settings["Ports"] is a dict like:
                # {'6901/tcp': [{'HostIp': '0.0.0.0', 'HostPort': '6901'}, ...], ...}
                for container_port, port_mappings in network_settings["Ports"].items():
                    if port_mappings:  # Check if there are any port mappings
                        # Take the first mapping (usually the IPv4 one)
                        for mapping in port_mappings:
                            if mapping.get("HostPort"):
                                ports[container_port] = mapping["HostPort"]
                                break  # Use the first valid mapping

            return {
                "name": name,
                "status": status,
                "ip_address": ip_address or "127.0.0.1",  # Use localhost if no IP
                "ports": ports,
                "image": container_info["Config"]["Image"],
                "provider": "docker",
                "container_id": container_info["Id"][:12],  # Short ID
                "created": container_info["Created"],
                "started": state.get("StartedAt", ""),
            }

        except Exception as e:
            logger.error(f"Error getting VM info for {name}: {e}")
            import traceback

            traceback.print_exc()
            return {"name": name, "status": "error", "error": str(e), "provider": "docker"}

    async def list_vms(self) -> List[Dict[str, Any]]:
        """List all Docker containers managed by this provider."""
        try:
            # List all containers (running and stopped) with the CUA image
            cmd = ["docker", "ps", "-a", "--filter", f"ancestor={self.image}", "--format", "json"]
            result = subprocess.run(cmd, capture_output=True, text=True, check=True)

            containers = []
            if result.stdout.strip():
                for line in result.stdout.strip().split("\n"):
                    if line.strip():
                        container_data = json.loads(line)
                        vm_info = await self.get_vm(container_data["Names"])
                        containers.append(vm_info)

            return containers

        except subprocess.CalledProcessError as e:
            logger.error(f"Error listing containers: {e.stderr}")
            return []
        except Exception as e:
            logger.error(f"Error listing VMs: {e}")
            import traceback

            traceback.print_exc()
            return []

    async def run_vm(
        self, image: str, name: str, run_opts: Dict[str, Any], storage: Optional[str] = None
    ) -> Dict[str, Any]:
        """Run a VM with the given options.

        Args:
            image: Name/tag of the Docker image to use
            name: Name of the container to run
            run_opts: Options for running the VM, including:
                - memory: Memory limit (e.g., "4GB", "2048MB")
                - cpu: CPU limit (e.g., 2 for 2 cores)
                - vnc_port: Specific port for VNC interface
                - api_port: Specific port for computer-server API

        Returns:
            Dictionary with VM status information
        """
        try:
            # Check if container already exists
            existing_vm = await self.get_vm(name, storage)
            if existing_vm["status"] == "running":
                logger.info(f"Container {name} is already running")
                return existing_vm
            elif existing_vm["status"] in ["stopped", "paused"]:
                if self.ephemeral:
                    # Delete existing container
                    logger.info(f"Deleting existing container {name}")
                    delete_cmd = ["docker", "rm", name]
                    result = subprocess.run(delete_cmd, capture_output=True, text=True, check=True)
                else:
                    # Start existing container
                    logger.info(f"Starting existing container {name}")
                    start_cmd = ["docker", "start", name]
                    result = subprocess.run(start_cmd, capture_output=True, text=True, check=True)

                    # Wait for container to be ready
                    await self._wait_for_container_ready(name)
                    return await self.get_vm(name, storage)

            # Use provided image or default
            docker_image = image if image != "default" else self.image

            # Build docker run command
            cmd = ["docker", "run", "-d", "--name", name]

            # Add memory limit if specified
            if "memory" in run_opts:
                memory_limit = self._parse_memory(run_opts["memory"])
                cmd.extend(["--memory", memory_limit])

            # Add CPU limit if specified
            if "cpu" in run_opts:
                cpu_count = str(run_opts["cpu"])
                cmd.extend(["--cpus", cpu_count])

            # Add port mappings
            vnc_port = run_opts.get("vnc_port", self.vnc_port)
            api_port = run_opts.get("api_port", self.api_port)

            if vnc_port:
                cmd.extend(["-p", f"{vnc_port}:6901"])  # VNC port
            if api_port:
                cmd.extend(["-p", f"{api_port}:8000"])  # computer-server API port

            # Add volume mounts if storage is specified
            storage_path = storage or self.storage
            if storage_path and storage_path != "ephemeral":
                # Mount storage directory using detected home directory
                cmd.extend(["-v", f"{storage_path}:{self._home_dir}/storage"])

            # Add shared path if specified
            if self.shared_path:
                # Mount shared directory using detected home directory
                cmd.extend(["-v", f"{self.shared_path}:{self._home_dir}/shared"])

            # Add environment variables
            cmd.extend(["-e", "VNC_PW=password"])  # Set VNC password
            cmd.extend(["-e", "VNCOPTIONS=-disableBasicAuth"])  # Disable VNC basic auth

            # Apply display resolution if provided (e.g., "1024x768")
            display_resolution = run_opts.get("display")
            if (
                isinstance(display_resolution, dict)
                and "width" in display_resolution
                and "height" in display_resolution
            ):
                cmd.extend(
                    [
                        "-e",
                        f"VNC_RESOLUTION={display_resolution['width']}x{display_resolution['height']}",
                    ]
                )

            # Add the image
            cmd.append(docker_image)

            logger.info(f"Running Docker container with command: {' '.join(cmd)}")

            # Run the container
            result = subprocess.run(cmd, capture_output=True, text=True, check=True)
            container_id = result.stdout.strip()

            logger.info(f"Container {name} started with ID: {container_id[:12]}")

            # Store container info
            self._container_id = container_id
            self._running_containers[name] = container_id

            # Wait for container to be ready
            await self._wait_for_container_ready(name)

            # Return VM info
            vm_info = await self.get_vm(name, storage)
            vm_info["container_id"] = container_id[:12]

            return vm_info

        except subprocess.CalledProcessError as e:
            error_msg = f"Failed to run container {name}: {e.stderr}"
            logger.error(error_msg)
            return {"name": name, "status": "error", "error": error_msg, "provider": "docker"}
        except Exception as e:
            error_msg = f"Error running VM {name}: {e}"
            logger.error(error_msg)
            return {"name": name, "status": "error", "error": error_msg, "provider": "docker"}

    async def _wait_for_container_ready(self, container_name: str, timeout: int = 60) -> bool:
        """Wait for the Docker container to be fully ready.

        Args:
            container_name: Name of the Docker container to check
            timeout: Maximum time to wait in seconds (default: 60 seconds)

        Returns:
            True if the container is running and ready
        """
        logger.info(f"Waiting for container {container_name} to be ready...")

        start_time = time.time()
        while time.time() - start_time < timeout:
            try:
                # Check if container is running
                vm_info = await self.get_vm(container_name)
                if vm_info["status"] == "running":
                    logger.info(f"Container {container_name} is running")

                    # Additional check: try to connect to computer-server API
                    # This is optional - we'll just wait a bit more for services to start
                    await asyncio.sleep(5)
                    return True

            except Exception as e:
                logger.debug(f"Container {container_name} not ready yet: {e}")

            await asyncio.sleep(2)

        logger.warning(f"Container {container_name} did not become ready within {timeout} seconds")
        return False

    async def stop_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Stop a running VM by stopping the Docker container."""
        try:
            logger.info(f"Stopping container {name}")

            # Stop the container
            cmd = ["docker", "stop", name]
            result = subprocess.run(cmd, capture_output=True, text=True, check=True)

            # Remove from running containers tracking
            if name in self._running_containers:
                del self._running_containers[name]

            logger.info(f"Container {name} stopped successfully")

            # Delete container if ephemeral=True
            if self.ephemeral:
                cmd = ["docker", "rm", name]
                result = subprocess.run(cmd, capture_output=True, text=True, check=True)

            return {
                "name": name,
                "status": "stopped",
                "message": "Container stopped successfully",
                "provider": "docker",
            }

        except subprocess.CalledProcessError as e:
            error_msg = f"Failed to stop container {name}: {e.stderr}"
            logger.error(error_msg)
            return {"name": name, "status": "error", "error": error_msg, "provider": "docker"}
        except Exception as e:
            error_msg = f"Error stopping VM {name}: {e}"
            logger.error(error_msg)
            return {"name": name, "status": "error", "error": error_msg, "provider": "docker"}

    async def restart_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        raise NotImplementedError("DockerProvider does not support restarting VMs.")

    async def update_vm(
        self, name: str, update_opts: Dict[str, Any], storage: Optional[str] = None
    ) -> Dict[str, Any]:
        """Update VM configuration.

        Note: Docker containers cannot be updated while running.
        This method will return an error suggesting to recreate the container.
        """
        return {
            "name": name,
            "status": "error",
            "error": "Docker containers cannot be updated while running. Please stop and recreate the container with new options.",
            "provider": "docker",
        }

    async def get_ip(self, name: str, storage: Optional[str] = None, retry_delay: int = 2) -> str:
        """Get the IP address of a VM, waiting indefinitely until it's available.

        Args:
            name: Name of the VM to get the IP for
            storage: Optional storage path override
            retry_delay: Delay between retries in seconds (default: 2)

        Returns:
            IP address of the VM when it becomes available
        """
        logger.info(f"Getting IP address for container {name}")

        total_attempts = 0
        while True:
            total_attempts += 1

            try:
                vm_info = await self.get_vm(name, storage)

                if vm_info["status"] == "error":
                    raise Exception(
                        f"VM is in error state: {vm_info.get('error', 'Unknown error')}"
                    )

                # TODO: for now, return localhost
                # it seems the docker container is not accessible from the host
                # on WSL2, unless you port forward? not sure
                if True:
                    logger.warning("Overriding container IP with localhost")
                    return "localhost"

                # Check if we got a valid IP
                ip = vm_info.get("ip_address", None)
                if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                    logger.info(f"Got valid container IP address: {ip}")
                    return ip

                # For Docker containers, we can also use localhost if ports are mapped
                if vm_info["status"] == "running" and vm_info.get("ports"):
                    logger.info("Container is running with port mappings, using localhost")
                    return "127.0.0.1"

                # Check the container status
                status = vm_info.get("status", "unknown")

                if status == "stopped":
                    logger.info(f"Container status is {status}, but still waiting for it to start")
                elif status != "running":
                    logger.info(f"Container is not running yet (status: {status}). Waiting...")
                else:
                    logger.info("Container is running but no valid IP address yet. Waiting...")

            except Exception as e:
                logger.warning(f"Error getting container {name} IP: {e}, continuing to wait...")

            # Wait before next retry
            await asyncio.sleep(retry_delay)

            # Add progress log every 10 attempts
            if total_attempts % 10 == 0:
                logger.info(
                    f"Still waiting for container {name} IP after {total_attempts} attempts..."
                )

    async def __aenter__(self):
        """Async context manager entry."""
        logger.debug("Entering DockerProvider context")
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit.

        This method handles cleanup of running containers if needed.
        """
        logger.debug(f"Exiting DockerProvider context, handling exceptions: {exc_type}")
        try:
            # Optionally stop running containers on context exit
            # For now, we'll leave containers running as they might be needed
            # Users can manually stop them if needed
            pass
        except Exception as e:
            logger.error(f"Error during DockerProvider cleanup: {e}")
            if exc_type is None:
                raise
        return False
