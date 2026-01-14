"""Task runner for 2-container architecture.

Orchestrates running tasks with separate agent and environment containers
communicating via Docker network.
"""

import asyncio
import os
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional

from ..sessions.providers.local_environment import check_image_exists, pull_image
from .docker_utils import (
    ContainerInfo,
    allocate_ports,
    create_network,
    create_overlay_copy,
    full_cleanup,
    generate_task_id,
    get_container_logs,
    remove_container,
    remove_image,
    remove_network,
    start_container,
    stop_container,
    wait_for_container,
)

# =============================================================================
# Configuration
# =============================================================================

# Environment type configurations
ENV_CONFIGS = {
    "linux-docker": {
        "image": "trycua/cua-xfce:latest",
        "internal_vnc_port": 6901,
        "internal_api_port": 8000,
        "requires_kvm": False,
        "os_type": "linux",
        "use_overlays": False,  # Stateless container, no disk to protect
    },
    "linux-qemu": {
        "image": "trycua/cua-qemu-linux:latest",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "os_type": "linux",
        "use_overlays": True,  # Protect golden QCOW2 disk
    },
    "windows-qemu": {
        "image": "trycua/cua-qemu-windows:latest",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "os_type": "windows",
        "use_overlays": True,  # Protect golden QCOW2 disk
    },
    "android-qemu": {
        "image": "trycua/cua-qemu-android:latest",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "os_type": "android",
        "use_overlays": True,  # Protect golden QCOW2 disk
    },
}

# Default agent image
DEFAULT_AGENT_IMAGE = "cua-bench:latest"


# XDG data directory
def get_data_dir() -> Path:
    """Get XDG data directory for cua-bench."""
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench"


def get_image_path(image_name: str) -> Path:
    """Get path to a stored image (QCOW2 disk for QEMU, etc)."""
    return get_data_dir() / "images" / image_name


def get_overlays_path() -> Path:
    """Get path to overlays directory for task overlay storage."""
    return get_data_dir() / "overlays"


# =============================================================================
# Data Types
# =============================================================================


@dataclass
class TaskResult:
    """Result of a task execution."""

    success: bool
    exit_code: int
    agent_logs: str
    env_logs: str
    output_dir: Optional[str] = None
    error: Optional[str] = None


# =============================================================================
# Task Runner
# =============================================================================


class TaskRunner:
    """Orchestrates 2-container task execution.

    Architecture:
    - Creates isolated Docker network per task
    - Creates task overlay to protect golden image (QEMU types)
    - Starts environment container (base image with QCOW2 disk)
    - Starts agent container (runs solver)
    - Agent connects to env via network hostname
    - Waits for agent completion
    - Collects results and cleans up (including overlay)
    """

    def __init__(
        self,
        agent_image: str = DEFAULT_AGENT_IMAGE,
        env_hostname: str = "cua-env",
        agent_hostname: str = "cua-agent",
    ):
        """Initialize TaskRunner.

        Args:
            agent_image: Docker image for agent container
            env_hostname: Hostname for environment container in network
            agent_hostname: Hostname for agent container in network
        """
        self.agent_image = agent_image
        self.env_hostname = env_hostname
        self.agent_hostname = agent_hostname

        # Track running containers for cleanup
        self._running_tasks: Dict[str, dict] = {}

    # =========================================================================
    # Task Overlay Management
    # =========================================================================

    def _create_task_overlay(self, task_id: str) -> Path:
        """Create a CoW overlay directory for a task.

        The QEMU container will use this directory for QCOW2 overlays,
        keeping the golden image pristine.

        Args:
            task_id: Unique task ID for isolation

        Returns:
            Path to the task overlay directory
        """
        overlay_path = get_overlays_path() / task_id

        # Clean up any existing overlay to ensure fresh state
        if overlay_path.exists():
            shutil.rmtree(overlay_path)

        overlay_path.mkdir(parents=True, exist_ok=True)
        return overlay_path

    def _cleanup_task_overlay(self, task_id: str) -> None:
        """Remove task overlay directory.

        Args:
            task_id: Task ID to clean up
        """
        overlay_path = get_overlays_path() / task_id
        if overlay_path.exists():
            shutil.rmtree(overlay_path)

    async def run_task(
        self,
        env_path: Path,
        task_index: int,
        env_type: str,
        golden_name: Optional[str] = None,
        *,
        # Agent configuration
        agent: Optional[str] = None,
        agent_image: Optional[str] = None,
        agent_command: Optional[List[str]] = None,
        agent_import_path: Optional[str] = None,
        model: Optional[str] = None,
        max_steps: int = 100,
        oracle: bool = False,
        # Environment configuration
        memory: str = "8G",
        cpus: str = "8",
        # Debugging
        vnc_port: Optional[int] = None,
        api_port: Optional[int] = None,
        # Output
        output_dir: Optional[str] = None,
        stream_agent_logs: bool = False,
        # Timeout
        timeout: Optional[int] = None,
        # Cleanup options
        cleanup_before: bool = True,
        remove_images_after: bool = False,
        # Provider type
        provider_type: Optional[str] = None,
    ) -> TaskResult:
        """Run a task with 2-container architecture.

        Args:
            env_path: Path to task environment directory
            task_index: Task index to run
            env_type: Environment type (linux-docker, windows-qemu, etc.)
            image_name: Image name to use (defaults to env_type). See: cb image list
            agent: Agent name (for built-in agents)
            agent_image: Docker image for agent container (overrides default)
            agent_command: Custom command for agent container
            agent_import_path: Custom agent import path
            model: Model to use
            max_steps: Maximum agent steps
            oracle: Run oracle solution instead of agent
            memory: Memory for environment (QEMU only)
            cpus: CPUs for environment (QEMU only)
            vnc_port: Host port to map VNC (for debugging)
            api_port: Host port to map API (for debugging)
            output_dir: Output directory for results
            stream_agent_logs: Stream agent logs to <output_dir>/run.log in real-time (default: False)
            timeout: Timeout in seconds (None = no timeout)
            cleanup_before: Clean up stale containers before starting (default: True)
            remove_images_after: Remove Docker images after task (default: False)
                Note: This removes Docker images but NOT base VM disk images.
            provider_type: Provider type ("simulated", "webtop", "native", "computer", None).
                If "simulated" or "webtop", the agent container will use a local
                Playwright session instead of connecting to a remote environment.

        Returns:
            TaskResult with execution details
        """
        # Cleanup stale containers from previous runs
        if cleanup_before:
            await full_cleanup()

        # Determine if this is a simulated provider
        is_simulated = provider_type in ("simulated", "webtop")

        # Validate env_type (skip validation for simulated since we don't use it)
        if not is_simulated and env_type not in ENV_CONFIGS:
            raise ValueError(
                f"Unknown env_type: {env_type}. Valid types: {list(ENV_CONFIGS.keys())}"
            )

        config = ENV_CONFIGS.get(env_type, {}) if not is_simulated else {}
        golden_name = golden_name or env_type if not is_simulated else "simulated"

        # Generate unique task ID
        task_id = generate_task_id()
        network_name = f"cua-task-{task_id}"
        env_container_name = f"cua-env-{task_id}"
        agent_container_name = f"cua-agent-{task_id}"

        # Create task overlay for QEMU types (protects golden image)
        overlay_path: Optional[Path] = None
        if not is_simulated and config.get("use_overlays"):
            overlay_path = self._create_task_overlay(task_id)

        # Track task for cleanup
        self._running_tasks[task_id] = {
            "network": network_name,
            "env_container": env_container_name if not is_simulated else None,
            "agent_container": agent_container_name,
            "env_image": config.get("image") if not is_simulated else None,
            "agent_image": self.agent_image,
            "remove_images": remove_images_after,
            "overlay_path": overlay_path,
            "is_simulated": is_simulated,
        }

        # Track log streaming process for cleanup
        log_stream_process = None

        try:
            # 1. Create network
            await create_network(network_name)

            # 2. Start environment container (skip for simulated providers)
            if not is_simulated:
                await self._start_env_container(
                    task_id=task_id,
                    network_name=network_name,
                    env_type=env_type,
                    golden_name=golden_name,
                    config=config,
                    memory=memory,
                    cpus=cpus,
                    vnc_port=vnc_port,
                    api_port=api_port,
                    overlay_path=overlay_path,
                )

            # 3. Start agent container (agent handles waiting for env)
            await self._start_agent_container(
                task_id=task_id,
                network_name=network_name,
                env_path=env_path,
                task_index=task_index,
                config=config,
                agent=agent,
                agent_image=agent_image,
                agent_command=agent_command,
                agent_import_path=agent_import_path,
                model=model,
                max_steps=max_steps,
                oracle=oracle,
                output_dir=output_dir,
                is_simulated=is_simulated,
            )

            # 3.5. Start streaming agent logs to file if requested
            if stream_agent_logs and output_dir:
                from .docker_utils import stream_container_logs_to_file

                log_file = Path(output_dir) / "run.log"
                log_stream_process = await stream_container_logs_to_file(
                    agent_container_name, log_file
                )

            # 4. Wait for agent to complete
            try:
                exit_code = await wait_for_container(
                    agent_container_name,
                    timeout=timeout,
                )
            except asyncio.TimeoutError:
                return TaskResult(
                    success=False,
                    exit_code=-1,
                    agent_logs=await get_container_logs(agent_container_name, tail=100),
                    env_logs=await get_container_logs(env_container_name, tail=100),
                    output_dir=output_dir,
                    error="Task timed out",
                )

            # 5. Collect logs
            agent_logs = await get_container_logs(agent_container_name, tail=500)
            env_logs = await get_container_logs(env_container_name, tail=100)

            return TaskResult(
                success=exit_code == 0,
                exit_code=exit_code,
                agent_logs=agent_logs,
                env_logs=env_logs,
                output_dir=output_dir,
            )

        except Exception as e:
            # Collect logs on error
            agent_logs = ""
            env_logs = ""
            try:
                agent_logs = await get_container_logs(agent_container_name, tail=100)
            except Exception:
                pass
            try:
                env_logs = await get_container_logs(env_container_name, tail=100)
            except Exception:
                pass

            return TaskResult(
                success=False,
                exit_code=-1,
                agent_logs=agent_logs,
                env_logs=env_logs,
                output_dir=output_dir,
                error=str(e),
            )

        finally:
            # 6. Stop log streaming if active
            if log_stream_process:
                try:
                    log_stream_process.terminate()
                    await log_stream_process.wait()
                except Exception:
                    pass

            # 7. Cleanup
            await self._cleanup_task(task_id)

    async def run_task_interactively(
        self,
        env_type: str,
        golden_name: Optional[str] = None,
        *,
        # Task configuration
        env_path: Optional[Path] = None,
        task_index: int = 0,
        # Environment configuration
        memory: str = "8G",
        cpus: str = "8",
        # Port allocation
        vnc_port: Optional[int] = None,
        api_port: Optional[int] = None,
        auto_allocate_ports: bool = True,
        # Cleanup options
        cleanup_before: bool = True,
    ) -> tuple[str, str, callable, Optional[dict]]:
        """Start an environment container interactively (without agent).

        This method starts only the environment container with VNC and API ports
        exposed to the host, allowing manual interaction or agent connection.
        If env_path is provided, it will also load the task and run the setup.

        Args:
            env_type: Environment type (linux-docker, windows-qemu, etc.)
            golden_name: Image name to use (defaults to env_type)
            env_path: Path to task directory (optional, for running task setup)
            task_index: Task index to run (default: 0)
            memory: Memory for environment (QEMU only)
            cpus: CPUs for environment (QEMU only)
            vnc_port: Host port to map VNC (None = auto-allocate)
            api_port: Host port to map API (None = auto-allocate)
            auto_allocate_ports: Auto-allocate ports if not specified (default: True)
            cleanup_before: Clean up stale containers before starting (default: True)

        Returns:
            Tuple of (vnc_url, api_url, cleanup_func, task_config, env, session)
            - vnc_url: URL to access VNC (e.g., http://localhost:8006)
            - api_url: URL to access API (e.g., http://localhost:5000)
            - cleanup_func: Async function to call when done to cleanup resources
            - task_config: Task configuration dict (None if env_path not provided)
            - env: Environment object (None if env_path not provided)
            - session: RemoteDesktopSession object (None if env_path not provided)

        Example:
            ```python
            runner = TaskRunner()
            vnc_url, api_url, cleanup, task_cfg, env, session = await runner.run_task_interactively(
                "linux-docker",
                env_path=Path("./my_task"),
                task_index=0
            )
            print(f"VNC: {vnc_url}")
            print(f"Task: {task_cfg.get('description')}")
            # ... do interactive work ...
            # Evaluate before cleanup
            if env and env.evaluate_task_fn:
                result = await env.evaluate_task_fn(task_cfg['_task_cfg'], session)
                print(f"Result: {result}")
            await cleanup()
            ```
        """
        # Cleanup stale containers from previous runs
        if cleanup_before:
            await full_cleanup()

        # Validate env_type
        if env_type not in ENV_CONFIGS:
            raise ValueError(
                f"Unknown env_type: {env_type}. Valid types: {list(ENV_CONFIGS.keys())}"
            )

        config = ENV_CONFIGS[env_type]
        golden_name = golden_name or env_type

        # Auto-allocate ports if requested and not specified
        if auto_allocate_ports and (vnc_port is None or api_port is None):
            allocated_vnc, allocated_api = allocate_ports(
                vnc_default=8006,
                api_default=5000,
            )
            vnc_port = vnc_port or allocated_vnc
            api_port = api_port or allocated_api

        # Generate unique task ID
        task_id = generate_task_id()
        network_name = f"cua-task-{task_id}"
        env_container_name = f"cua-env-{task_id}"

        # Create task overlay for QEMU types (protects golden image)
        overlay_path: Optional[Path] = None
        if config.get("use_overlays"):
            overlay_path = self._create_task_overlay(task_id)

        # Track task for cleanup
        self._running_tasks[task_id] = {
            "network": network_name,
            "env_container": env_container_name,
            "agent_container": None,  # No agent in interactive mode
            "env_image": config["image"],
            "agent_image": None,
            "remove_images": False,
            "overlay_path": overlay_path,
        }

        # Create network
        await create_network(network_name)

        # Start environment container
        await self._start_env_container(
            task_id=task_id,
            network_name=network_name,
            env_type=env_type,
            golden_name=golden_name,
            config=config,
            memory=memory,
            cpus=cpus,
            vnc_port=vnc_port,
            api_port=api_port,
            overlay_path=overlay_path,
        )

        # Build URLs
        vnc_url = f"http://localhost:{vnc_port}" if vnc_port else None
        api_url = f"http://localhost:{api_port}" if api_port else None

        # Load task and run setup if env_path provided
        task_config = None
        env_obj = None
        session_obj = None

        if env_path and env_path.exists() and api_url:
            try:
                import time

                from cua_bench import make
                from cua_bench.computers.remote import RemoteDesktopSession

                # Load task definition
                env_obj = make(str(env_path))

                # Get tasks
                if env_obj.tasks_config_fn:
                    tasks = env_obj.tasks_config_fn()
                    if tasks and len(tasks) > task_index:
                        task_cfg = tasks[task_index]
                        task_config = {
                            "description": (
                                task_cfg.description if hasattr(task_cfg, "description") else None
                            ),
                            "metadata": task_cfg.metadata if hasattr(task_cfg, "metadata") else {},
                            "_task_cfg": task_cfg,  # Store for evaluation
                        }

                        # Create remote session
                        session_obj = RemoteDesktopSession(
                            api_url=api_url,
                            vnc_url=vnc_url or "",
                            os_type=config.get("os_type", "linux"),
                        )

                        # Wait for environment to be ready
                        boot_timeout = 300 if config.get("os_type") == "windows" else 120
                        if await session_obj.wait_until_ready(timeout=boot_timeout):
                            # Run setup function
                            _t0 = time.perf_counter()
                            if env_obj.setup_task_fn:
                                await env_obj.setup_task_fn(task_cfg, session_obj)
                            screenshot = await session_obj.screenshot()
                            _elapsed = time.perf_counter() - _t0

                            # Store setup time in task_config for display
                            task_config["_setup_time"] = _elapsed
                            task_config["_screenshot_size"] = len(screenshot)
            except Exception as e:
                import traceback

                print(f"Warning: Failed to run task setup: {e}")
                traceback.print_exc()

        # Create cleanup function
        async def cleanup():
            """Cleanup the interactive environment."""
            await self._cleanup_task(task_id)

        return vnc_url, api_url, cleanup, task_config, env_obj, session_obj

    async def _start_env_container(
        self,
        task_id: str,
        network_name: str,
        env_type: str,
        golden_name: str,
        config: dict,
        memory: str,
        cpus: str,
        vnc_port: Optional[int],
        api_port: Optional[int],
        overlay_path: Optional[Path] = None,
    ) -> ContainerInfo:
        """Start the environment container.

        For QEMU types with use_overlays=True:
        - Golden image is mounted read-only at /golden
        - Task overlay is mounted writable at /storage
        - QEMU creates QCOW2 overlay on top, protecting the golden image
        """
        container_name = f"cua-env-{task_id}"

        # Build environment variables
        env_vars = {}
        if config["requires_kvm"]:
            env_vars["RAM_SIZE"] = memory
            env_vars["CPU_CORES"] = cpus
            if not os.path.exists("/dev/kvm"):
                env_vars["KVM"] = "N"

        # Build volumes
        volumes = []
        if config["requires_kvm"]:
            image_path = get_image_path(golden_name)
            if image_path.exists():
                if config.get("use_overlays") and overlay_path:
                    # Copy golden image to overlay (shared utility)
                    create_overlay_copy(image_path, overlay_path, verbose=False)
                    volumes.append(f"{overlay_path}:/storage")
                else:
                    # No overlay - mount image directly (not recommended)
                    volumes.append(f"{image_path}:/storage")

        # Build port mappings (optional, for debugging)
        ports = {}
        if vnc_port:
            ports[vnc_port] = config["internal_vnc_port"]
        if api_port:
            ports[api_port] = config["internal_api_port"]

        # Build devices
        devices = []
        if config["requires_kvm"] and os.path.exists("/dev/kvm"):
            devices.append("/dev/kvm")

        # Start container
        return await start_container(
            image=config["image"],
            name=container_name,
            network=network_name,
            hostname=self.env_hostname,
            env_vars=env_vars,
            volumes=volumes,
            ports=ports if ports else None,
            privileged=config["requires_kvm"],
            devices=devices if devices else None,
            platform="linux/amd64" if config["requires_kvm"] else None,
            shm_size="16g" if config["requires_kvm"] else None,
            cap_add=["NET_ADMIN"] if config["requires_kvm"] else None,
            stop_timeout=120,
        )

    async def _build_agent_image(self, agent_path: Path, image_tag: str) -> bool:
        """Build Docker image for a local agent directory.

        Args:
            agent_path: Path to agent directory containing Dockerfile
            image_tag: Tag for the built image

        Returns:
            True if build succeeded, False otherwise
        """
        print(f"Building agent image: {image_tag}")
        print(f"  Source: {agent_path}")

        process = await asyncio.create_subprocess_exec(
            "docker",
            "build",
            "-t",
            image_tag,
            ".",
            cwd=agent_path,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            print("Failed to build agent image:")
            print(stderr.decode())
            return False

        print(f"Successfully built {image_tag}")
        return True

    async def _resolve_agent_image(
        self,
        agent: Optional[str],
        agent_image: Optional[str],
        agent_import_path: Optional[str],
    ) -> tuple[str, Optional[List[str]], Optional[str]]:
        """Resolve agent configuration to a Docker image.

        Args:
            agent: Agent name (for built-in agents)
            agent_image: Explicit Docker image
            agent_import_path: Path to local agent directory or import path

        Returns:
            Tuple of (image_name, command, updated_agent_import_path)
            - command is None for default solver behavior
            - updated_agent_import_path is None if using a local agent image
        """
        # 1. Explicit Docker image - use as-is
        if agent_image:
            return agent_image, None, agent_import_path

        # 2. Local agent directory with Dockerfile - build and use
        if agent_import_path:
            # Check if it's a path to a directory (supports both ./my-agent and my_agent.agent:Class)
            path_part = agent_import_path.split(":")[0]

            # Try as direct path first
            agent_path = Path(path_part)
            if not agent_path.exists():
                # Try converting module path to directory path
                agent_path = Path(path_part.replace(".", "/"))

            if agent_path.exists() and agent_path.is_dir():
                dockerfile = agent_path / "Dockerfile"
                if dockerfile.exists():
                    # Build the agent image
                    image_tag = f"cua-agent-{agent_path.name}:local"
                    success = await self._build_agent_image(agent_path.resolve(), image_tag)
                    if success:
                        # Use the built image, no command override needed
                        # (the Dockerfile has its own entrypoint)
                        return image_tag, None, None
                    else:
                        raise RuntimeError(f"Failed to build agent image from {agent_path}")

        # 3. Default - use the default agent image with solver command
        # Validate the image exists, attempt pull if not
        if not check_image_exists(self.agent_image):
            print(f"Agent image '{self.agent_image}' not found locally. Attempting to pull...")
            try:
                pull_image(self.agent_image)
                print(f"Successfully pulled {self.agent_image}")
            except Exception as e:
                raise RuntimeError(
                    f"Agent image '{self.agent_image}' not found and could not be pulled.\n\n"
                    f"Build it locally with:\n"
                    f"  cd /path/to/cua-bench && docker build -t {self.agent_image} .\n\n"
                    f"Or specify a custom agent with --agent-import-path\n\n"
                    f"Pull error: {e}"
                )
        return self.agent_image, None, agent_import_path

    async def _start_agent_container(
        self,
        task_id: str,
        network_name: str,
        env_path: Path,
        task_index: int,
        config: dict,
        agent: Optional[str],
        agent_image: Optional[str],
        agent_command: Optional[List[str]],
        agent_import_path: Optional[str],
        model: Optional[str],
        max_steps: int,
        oracle: bool,
        output_dir: Optional[str],
        is_simulated: bool = False,
    ) -> ContainerInfo:
        """Start the agent container.

        Supports three modes:
        1. Custom Docker image agent: Uses agent_image with optional agent_command
        2. Local agent directory: Auto-builds Dockerfile and uses built image
        3. Default image agent: Uses default image with solver.py command

        For custom Docker image agents, the container receives env vars:
        - CUA_ENV_API_URL: API endpoint for environment
        - CUA_ENV_VNC_URL: VNC endpoint for debugging
        - CUA_ENV_TYPE: OS type (linux/windows/android)
        - CUA_TASK_PATH: Path to mounted task config (/app/env)
        - CUA_TASK_INDEX: Task index to run
        - CUA_MAX_STEPS: Maximum steps for agent
        - API keys (ANTHROPIC_API_KEY, etc.)
        """
        container_name = f"cua-agent-{task_id}"

        # Resolve agent to Docker image (may build local agent)
        image, resolved_command, resolved_import_path = await self._resolve_agent_image(
            agent=agent,
            agent_image=agent_image,
            agent_import_path=agent_import_path,
        )

        # Update agent_import_path if we built a local agent
        if resolved_import_path is None:
            agent_import_path = None

        # Use resolved command if provided, otherwise use provided command
        if resolved_command is not None:
            agent_command = resolved_command

        # Build API URL using network hostname (not used for simulated, but set for consistency)
        api_url = (
            f"http://{self.env_hostname}:{config.get('internal_api_port', 5000)}" if config else ""
        )
        vnc_url = (
            f"http://{self.env_hostname}:{config.get('internal_vnc_port', 8006)}" if config else ""
        )

        # Build environment variables (available to all agent images)
        env_vars = {
            "CUA_ENV_API_URL": api_url,
            "CUA_ENV_VNC_URL": vnc_url,
            "CUA_ENV_TYPE": config.get("os_type", "linux") if config else "linux",
            "CUA_PROVIDER": "simulated" if is_simulated else "remote",
            "CUA_TASK_PATH": "/app/env",
            "CUA_TASK_INDEX": str(task_index),
            "BATCH_TASK_INDEX": str(task_index),  # Legacy compat
            "BATCH_TASK_COUNT": "1",
        }

        # Add model and max_steps to env if specified
        if model:
            env_vars["CUA_MODEL"] = model
        if max_steps:
            env_vars["CUA_MAX_STEPS"] = str(max_steps)

        # Pass through API keys from host
        api_keys = [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_CLOUD_PROJECT",
            "AZURE_OPENAI_API_KEY",
            "AZURE_OPENAI_ENDPOINT",
        ]
        for key in api_keys:
            value = os.environ.get(key)
            if value:
                env_vars[key] = value

        # Build volumes
        volumes = [
            f"{env_path.absolute()}:/app/env:ro",
        ]
        if output_dir:
            Path(output_dir).mkdir(parents=True, exist_ok=True)
            volumes.append(f"{Path(output_dir).absolute()}:/tmp/td_output")

        # Mount local cua_bench if running in development mode
        # This allows local code changes to be picked up by the agent container
        cua_bench_path = Path(__file__).parent.parent.absolute()
        if cua_bench_path.exists() and (cua_bench_path / "computers").exists():
            volumes.append(f"{cua_bench_path}:/app/cua_bench:ro")

        # Build command
        if agent_command:
            # Custom command for Docker image agents
            command = agent_command
        else:
            # Default: use cua_bench.batch.solver
            command = ["python", "-m", "cua_bench.batch.solver", "/app/env"]
            command.extend(["--task-index", str(task_index)])

            if oracle:
                pass  # Oracle is default
            else:
                command.append("--eval")
                if agent:
                    command.extend(["--agent", agent])
                if agent_import_path:
                    command.extend(["--agent-import-path", agent_import_path])
                if model:
                    command.extend(["--model", model])
                if max_steps:
                    command.extend(["--max-steps", str(max_steps)])

        # Start container (not detached - we want to wait for it)
        return await start_container(
            image=image,
            name=container_name,
            network=network_name,
            hostname=self.agent_hostname,
            env_vars=env_vars,
            volumes=volumes,
            command=command,
            detach=True,  # Detach so we can use docker wait
            remove_on_exit=False,  # Don't auto-remove, we need logs
        )

    async def _cleanup_task(self, task_id: str) -> None:
        """Clean up task resources (containers, network, overlay, optionally images).

        This is called in the finally block of run_task() to ensure cleanup
        happens even if the task fails or times out.
        """
        task_info = self._running_tasks.pop(task_id, None)
        if not task_info:
            return

        # Stop and remove containers (force to ensure cleanup)
        for container_name in [task_info["agent_container"], task_info["env_container"]]:
            try:
                await stop_container(container_name, force=True)
            except Exception:
                pass  # Container may already be stopped
            try:
                await remove_container(container_name)
            except Exception:
                pass  # Container may already be removed

        # Remove network
        try:
            await remove_network(task_info["network"])
        except Exception:
            pass  # Network may already be removed

        # Remove task overlay directory
        if task_info.get("overlay_path"):
            self._cleanup_task_overlay(task_id)

        # Optionally remove Docker images (NOT base VM disk images)
        if task_info.get("remove_images", False):
            # Remove environment image (e.g., trycua/cua-qemu-windows:latest)
            # Note: This removes the Docker image layer, NOT the base VM disk
            # The VM disk at ~/.local/share/cua-bench/images/ is preserved
            env_image = task_info.get("env_image")
            if env_image:
                await remove_image(env_image)

            # Remove agent image
            agent_image = task_info.get("agent_image")
            if agent_image:
                await remove_image(agent_image)

    async def cleanup_all(self) -> None:
        """Clean up all running tasks."""
        for task_id in list(self._running_tasks.keys()):
            await self._cleanup_task(task_id)

    @staticmethod
    async def force_cleanup() -> dict:
        """Force cleanup of all stale cua-bench containers and networks.

        Use this when containers are left behind from previous runs.

        Returns:
            Dict with counts: {"containers": N, "networks": N}
        """
        return await full_cleanup()
