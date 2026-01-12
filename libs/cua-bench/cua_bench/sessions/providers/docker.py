"""Docker session provider."""

import asyncio
import json
import os
import random
import socket
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional

from .base import SessionProvider


class DockerProvider(SessionProvider):
    """Docker-based session provider for local container execution."""

    def __init__(self):
        self.running_containers: Dict[str, str] = {}  # session_id -> container_id
        self.session_networks: Dict[str, str] = {}  # session_id -> network_name

    def _find_available_port(self, start_port: int = 8000, max_port: int = 65535) -> int:
        """Find an available port starting from a random port in the range."""
        port = random.randint(start_port, min(max_port - 1000, 60000))

        while port <= max_port:
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
                    sock.bind(("localhost", port))
                    return port
            except OSError:
                port += 1

        raise RuntimeError(f"No available ports found in range {start_port}-{max_port}")

    async def _ensure_shared_network(self) -> str:
        """Ensure the shared cua-bench network exists, creating it if necessary.

        Returns:
            The network name
        """
        network_name = "cua-bench_default"

        # Check if network already exists
        process = await asyncio.create_subprocess_exec(
            "docker",
            "network",
            "inspect",
            network_name,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()

        if process.returncode == 0:
            # Network already exists
            return network_name

        # Network doesn't exist, create it
        process = await asyncio.create_subprocess_exec(
            "docker",
            "network",
            "create",
            network_name,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            error_msg = stderr.decode() if stderr else "Unknown error"
            raise RuntimeError(f"Failed to create Docker network: {error_msg}")

        return network_name

    async def _remove_network(self, network_name: str) -> None:
        """Remove a Docker network."""
        process = await asyncio.create_subprocess_exec(
            "docker",
            "network",
            "rm",
            network_name,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        await process.communicate()

    def _find_two_available_ports(self) -> tuple[int, int]:
        """Find two available ports for API and noVNC."""
        api_port = self._find_available_port(8000, 65535)

        # Find second port, avoiding the first one
        novnc_port = api_port + 1
        while novnc_port <= 65535:
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
                    sock.bind(("localhost", novnc_port))
                    return api_port, novnc_port
            except OSError:
                novnc_port += 1

        raise RuntimeError("Could not find two available ports")

    async def _create_computer_container(
        self, task_config: Dict[str, Any], task_index: int, network_name: str
    ) -> Dict[str, Any]:
        """Create a computer container for a task.

        Args:
            task_config: Task configuration dict with os_type, width, height, background, etc.
            task_index: Index of this task
            network_name: Docker network to attach the container to

        Returns:
            Dict with container info (name, api_port, novnc_port, os_type)
        """
        # Generate random container name
        container_name = f"cb-task-{task_index}-{str(uuid.uuid4())[:5]}"

        # Find two available ports
        api_port, novnc_port = self._find_two_available_ports()

        # Get display resolution
        width = task_config.get("width", 1024)
        height = task_config.get("height", 768)

        # Build docker run command
        docker_cmd = [
            "docker",
            "run",
            "-d",
            "--name",
            container_name,
            "--network",
            network_name,
            "-p",
            f"{novnc_port}:6901",  # VNC port
            "-p",
            f"{api_port}:8000",  # computer-server API port
            "-e",
            "VNC_PW=password",
            "-e",
            "VNCOPTIONS=-disableBasicAuth",
            "-e",
            f"VNC_RESOLUTION={width}x{height}",
            "trycua/cua-xfce:latest",
        ]

        # Start container
        process = await asyncio.create_subprocess_exec(
            *docker_cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            error_msg = stderr.decode() if stderr else "Unknown error"
            raise RuntimeError(f"Failed to start computer container: {error_msg}")

        return {
            "name": container_name,
            "api_port": api_port,
            "novnc_port": novnc_port,
            "os_type": task_config.get("os_type", "linux"),
        }

    async def _cleanup_stale_child_containers(self) -> None:
        """Clean up child containers from sessions that are not starting or running."""
        from ..manager import list_sessions

        # Get all sessions
        all_sessions = list_sessions(provider="docker")

        # Collect child containers from non-starting/running sessions
        containers_to_cleanup = []

        for session in all_sessions:
            session_id = session.get("session_id")
            if not session_id:
                continue

            # Get current status
            status_info = await self.get_session_status(session_id)
            status = status_info.get("status", "unknown")

            # If session is not starting or running, mark its child containers for cleanup
            if status not in ["running", "restarting"]:
                child_containers = session.get("child_containers", [])
                for child in child_containers:
                    containers_to_cleanup.append(child["name"])

        # Clean up the containers
        for container_name in containers_to_cleanup:
            try:
                # Check if container exists before trying to stop it
                inspect_process = await asyncio.create_subprocess_exec(
                    "docker",
                    "inspect",
                    container_name,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                )
                await inspect_process.communicate()

                if inspect_process.returncode == 0:
                    await self._stop_container(container_name)
            except Exception:
                # Silently ignore errors - container might already be gone
                pass

    async def start_session(
        self,
        session_id: str,
        env_path: Path,
        container_script: str,
        image_uri: Optional[str] = None,
        output_dir: Optional[str] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """Start a new Docker container session.

        Args:
            session_id: Unique identifier for the session
            env_path: Path to the environment directory
            container_script: Script to run in the container
            image_uri: Docker image to use (default: cua-bench:latest)
            output_dir: Directory to save outputs
            **kwargs: Additional arguments (task_index, task_count, etc.)

        Returns:
            Dict containing session metadata
        """
        # Clean up stale child containers before starting new session
        await self._cleanup_stale_child_containers()

        image = image_uri or "cua-bench:latest"
        task_index = kwargs.get("task_index", 0)
        task_count = kwargs.get("task_count", 1)

        # Use shared Docker network for all sessions
        network_name = await self._ensure_shared_network()

        # Load environment to check if tasks need computer containers
        child_containers: List[Dict[str, Any]] = []

        try:
            # Import cua_bench to load the environment
            import sys

            sys.path.insert(0, str(env_path.parent))

            from cua_bench import make

            env = make(str(env_path))

            # Get task configurations
            if env.tasks_config_fn:
                tasks = env.tasks_config_fn()

                # Only check the specific task we're running (task_index)
                if task_index < len(tasks):
                    task = tasks[task_index]
                    # Task.computer is a dict with the computer configuration
                    # Only create computer container if provider is "computer"
                    if task.computer and isinstance(task.computer, dict):
                        provider_type = task.computer.get("provider", "")
                        if provider_type == "computer":
                            container_info = await self._create_computer_container(
                                task.computer.get("setup_config", {}), task_index, network_name
                            )
                            child_containers.append(container_info)

            await env.close()
        except Exception as e:
            print(f"Warning: Could not load environment to check for computer tasks: {e}")
            raise e

        # Build docker command
        docker_cmd = [
            "docker",
            "run",
            "-d",  # Run in detached mode
            "--name",
            f"cb-session-{session_id}",
            "--network",
            network_name,
            "-e",
            f"BATCH_TASK_INDEX={task_index}",
            "-e",
            f"BATCH_TASK_COUNT={task_count}",
            "-v",
            f"{env_path.absolute()}:/app/env:ro",
        ]

        # Pass through environment variables from host
        env_vars_to_pass = [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "AZURE_OPENAI_API_KEY",
            "AZURE_OPENAI_ENDPOINT",
        ]

        for env_var in env_vars_to_pass:
            value = os.environ.get(env_var)
            if value:
                docker_cmd.extend(["-e", f"{env_var}={value}"])

        # Add child container info as environment variable
        if child_containers:
            containers_json = json.dumps(child_containers)
            docker_cmd.extend(["-e", f"CUA_TASK_CONTAINERS={containers_json}"])

        # Add output directory mount - create one if not specified
        if not output_dir:
            import tempfile

            # Generate a short hex ID for the temp directory
            temp_hex = uuid.uuid4().hex[:8]
            output_dir = tempfile.mkdtemp(prefix=f"cb_logs_{temp_hex}_")
            print(f"Created temporary output directory: {output_dir}")

        output_path = Path(output_dir).absolute()
        output_path.mkdir(parents=True, exist_ok=True)
        docker_cmd.extend(["-v", f"{output_path}:/tmp/td_output"])

        docker_cmd.append(image)
        docker_cmd.extend(["/bin/sh", "-c", container_script])

        # Start container
        process = await asyncio.create_subprocess_exec(
            *docker_cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            error_msg = stderr.decode() if stderr else "Unknown error"
            # Clean up child containers on failure
            for container in child_containers:
                try:
                    await self._stop_container(container["name"])
                except Exception:
                    pass
            raise RuntimeError(f"Failed to start Docker container: {error_msg}")

        container_id = stdout.decode().strip()
        self.running_containers[session_id] = container_id

        return {
            "session_id": session_id,
            "container_id": container_id,
            "provider": "docker",
            "status": "running",
            "env_path": str(env_path),
            "output_dir": output_dir,
            "image": image,
            "task_index": task_index,
            "task_count": task_count,
            "child_containers": child_containers,
        }

    async def get_session_status(self, session_id: str) -> Dict[str, Any]:
        """Get the status of a Docker container session.

        Args:
            session_id: Session identifier

        Returns:
            Dict containing session status
        """
        # First check session storage
        from ..manager import get_session

        session = get_session(session_id)

        # Check if we have this session in our tracking
        container_id = self.running_containers.get(session_id)

        # If not in running_containers, check if it exists in storage
        if not container_id and session:
            container_id = session.get("container_id")

        # If we have a stored status but no container, check if we should update it
        if session and not container_id:
            stored_status = session.get("status")

            # For completed/failed, trust the stored status (with cached reward if available)
            if stored_status in ("completed", "failed"):
                result = {
                    "session_id": session_id,
                    "status": stored_status,
                    "from_storage": True,
                }
                # Include cached reward if available
                if "reward" in session:
                    result["reward"] = session["reward"]
                return result

            # For running/starting, check if process is still alive
            if stored_status in ("running", "starting"):
                pid = session.get("pid")
                if pid:
                    # Try using psutil if available
                    try:
                        import psutil

                        try:
                            process = psutil.Process(pid)
                            if process.is_running():
                                # Process still running
                                return {
                                    "session_id": session_id,
                                    "status": stored_status,
                                    "from_storage": True,
                                    "pid": pid,
                                }
                        except (psutil.NoSuchProcess, psutil.AccessDenied):
                            # Process not found - check log file for status
                            pass
                    except ImportError:
                        # psutil not available - fallback to checking log file
                        pass

                    # Process finished or psutil not available - check output to determine status
                    output_dir = session.get("output_dir")
                    if output_dir:
                        import re
                        from pathlib import Path

                        log_file = Path(output_dir) / "run.log"
                        if log_file.exists():
                            try:
                                with open(log_file, "r", encoding="utf-8") as f:
                                    logs = f.read()
                                    final_status = None
                                    reward = None

                                    if "✓ Task completed successfully!" in logs:
                                        final_status = "completed"
                                        # Try to extract reward
                                        match = re.search(
                                            r"✓ Evaluation result: \[([^\]]+)\]", logs
                                        )
                                        if match:
                                            try:
                                                reward = float(match.group(1))
                                            except ValueError:
                                                pass
                                    elif "✗ Task failed" in logs or "Error:" in logs:
                                        final_status = "failed"
                                        reward = 0.0

                                    if final_status:
                                        # Cache the final status and reward in runs.json
                                        from ..manager import update_session

                                        update_data = {"status": final_status}
                                        if reward is not None:
                                            update_data["reward"] = reward
                                        update_session(session_id, update_data)

                                        return {
                                            "session_id": session_id,
                                            "status": final_status,
                                            "reward": reward,
                                            "from_storage": True,
                                        }
                                    else:
                                        # Log file exists but no completion markers - still running
                                        return {
                                            "session_id": session_id,
                                            "status": stored_status,
                                            "from_storage": True,
                                            "pid": pid,
                                        }
                            except Exception:
                                # Error reading log file - assume still running
                                return {
                                    "session_id": session_id,
                                    "status": stored_status,
                                    "from_storage": True,
                                    "pid": pid,
                                }
                        else:
                            # Log file doesn't exist yet - process is still starting
                            return {
                                "session_id": session_id,
                                "status": stored_status,
                                "from_storage": True,
                                "pid": pid,
                            }
                    # Process finished but couldn't determine status
                    return {"session_id": session_id, "status": "unknown", "from_storage": True}

            # Other statuses - return as-is
            if stored_status:
                return {
                    "session_id": session_id,
                    "status": stored_status,
                    "from_storage": True,
                }

        # No session found at all
        if not container_id:
            return {"status": "not_found", "session_id": session_id}

        # Get container status using docker inspect
        process = await asyncio.create_subprocess_exec(
            "docker",
            "inspect",
            container_id,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            # Container doesn't exist in Docker - it was deleted
            return {"status": "deleted", "session_id": session_id}

        try:
            inspect_data = json.loads(stdout.decode())
            if not inspect_data:
                return {"status": "deleted", "session_id": session_id}

            container_info = inspect_data[0]
            state = container_info.get("State", {})

            status = "unknown"
            if state.get("Running"):
                status = "running"
            elif state.get("Paused"):
                status = "paused"
            elif state.get("Restarting"):
                status = "restarting"
            elif state.get("Dead"):
                status = "dead"
            elif state.get("Status") == "exited":
                exit_code = state.get("ExitCode", -1)
                if exit_code == 0:
                    status = "completed"
                elif exit_code == 137:  # SIGKILL
                    status = "stopped"
                else:
                    status = "failed"

            return {
                "session_id": session_id,
                "container_id": container_id,
                "status": status,
                "exit_code": state.get("ExitCode"),
                "started_at": state.get("StartedAt"),
                "finished_at": state.get("FinishedAt"),
            }
        except (json.JSONDecodeError, KeyError) as e:
            return {"status": "error", "session_id": session_id, "error": str(e)}

    async def _stop_container(self, container_name: str) -> None:
        """Stop and remove a container by name."""
        # Stop the container
        stop_process = await asyncio.create_subprocess_exec(
            "docker",
            "stop",
            container_name,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        await stop_process.communicate()

        # Remove the container
        rm_process = await asyncio.create_subprocess_exec(
            "docker",
            "rm",
            container_name,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        await rm_process.communicate()

    async def stop_session(self, session_id: str) -> None:
        """Stop and remove a Docker container session and its child containers.

        Args:
            session_id: Session identifier
        """
        # Get session metadata to find child containers and container_id
        from ..manager import get_session

        session = get_session(session_id)

        if not session:
            return

        container_id = session.get("container_id") or self.running_containers.get(session_id)

        # Stop child containers first
        if "child_containers" in session:
            for child in session["child_containers"]:
                try:
                    print(f"Stopping child container: {child['name']}")
                    await self._stop_container(child["name"])
                except Exception as e:
                    print(f"Warning: Failed to stop child container {child['name']}: {e}")

        # Stop the main container
        if container_id:
            await self._stop_container(container_id)

        # Note: We no longer remove the shared network as it's reused across sessions

        # Remove from tracking
        if session_id in self.running_containers:
            del self.running_containers[session_id]

    async def get_session_logs(self, session_id: str, tail: Optional[int] = None) -> str:
        """Get logs from a Docker container session.

        Args:
            session_id: Session identifier
            tail: Number of lines to return from the end (None for all)

        Returns:
            Log output as string
        """
        # Check session storage
        from ..manager import get_session

        session = get_session(session_id)

        # First check if we have this session in our tracking
        container_id = self.running_containers.get(session_id)

        # If not in running_containers, check if it exists in storage
        if not container_id and session:
            container_id = session.get("container_id")

        # If no container, try to read from log file
        if not container_id:
            if session:
                output_dir = session.get("output_dir")
                if output_dir:
                    from pathlib import Path

                    log_file = Path(output_dir) / "run.log"
                    if log_file.exists():
                        try:
                            with open(log_file, "r", encoding="utf-8") as f:
                                lines = f.readlines()
                                if tail is not None:
                                    lines = lines[-tail:]
                                return "".join(lines)
                        except Exception as e:
                            return f"Failed to read log file {log_file}: {e}"
                    else:
                        return f"Log file not found: {log_file}"
                else:
                    return f"No output directory for session {session_id}"
            else:
                return f"Session {session_id} not found"

        # Build docker logs command
        docker_cmd = ["docker", "logs"]
        if tail is not None:
            docker_cmd.extend(["--tail", str(tail)])
        docker_cmd.append(container_id)

        process = await asyncio.create_subprocess_exec(
            *docker_cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        stdout, stderr = await process.communicate()

        if process.returncode != 0:
            return f"Failed to get logs for session {session_id}: {stderr.decode()}"

        # Combine stdout and stderr
        logs = stdout.decode()
        if stderr:
            logs += "\n" + stderr.decode()

        return logs
