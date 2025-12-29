"""
Watchdog module for monitoring the Computer API server health.
Unix/Linux only - provides process management and restart capabilities.
"""

import asyncio
import fcntl
import json
import logging
import os
import platform
import subprocess
import sys
import time
from typing import Optional

import websockets

logger = logging.getLogger(__name__)


def instance_already_running(label="watchdog"):
    """
    Detect if an an instance with the label is already running, globally
    at the operating system level.

    Using `os.open` ensures that the file pointer won't be closed
    by Python's garbage collector after the function's scope is exited.

    The lock will be released when the program exits, or could be
    released if the file pointer were closed.
    """

    lock_file_pointer = os.open(f"/tmp/instance_{label}.lock", os.O_WRONLY | os.O_CREAT)

    try:
        fcntl.lockf(lock_file_pointer, fcntl.LOCK_EX | fcntl.LOCK_NB)
        already_running = False
    except IOError:
        already_running = True

    return already_running


class Watchdog:
    """Watchdog class to monitor server health via WebSocket connection.
    Unix/Linux only - provides restart capabilities.
    """

    def __init__(self, cli_args: Optional[dict] = None, ping_interval: int = 30):
        """
        Initialize the watchdog.

        Args:
            cli_args: Dictionary of CLI arguments to replicate when restarting
            ping_interval: Interval between ping checks in seconds
        """
        # Check if running on Unix/Linux
        if platform.system() not in ["Linux", "Darwin"]:
            raise RuntimeError("Watchdog is only supported on Unix/Linux systems")

        # Store CLI arguments for restart
        self.cli_args = cli_args or {}
        self.host = self.cli_args.get("host", "localhost")
        self.port = self.cli_args.get("port", 8000)
        self.ping_interval = ping_interval
        self.container_name = os.environ.get("CONTAINER_NAME")
        self.running = False
        self.restart_enabled = True

    @property
    def ws_uri(self) -> str:
        """Get the WebSocket URI using the current IP address.

        Returns:
            WebSocket URI for the Computer API Server
        """
        if not self.container_name:
            return "ws://localhost:8000/ws"

        # Try .sandbox.cua.ai first, fallback to .containers.cloud.trycua.com
        return f"wss://{self.container_name}.sandbox.cua.ai:8443/ws"

    @property
    def ws_uri_fallback(self) -> str:
        """Get the fallback WebSocket URI using legacy hostname.

        Returns:
            Fallback WebSocket URI for the Computer API Server
        """
        if not self.container_name:
            return "ws://localhost:8000/ws"

        return f"wss://{self.container_name}.containers.cloud.trycua.com:8443/ws"

    async def ping(self) -> bool:
        """
        Test connection to the WebSocket endpoint.

        Returns:
            True if connection successful, False otherwise
        """
        # Create a simple ping message
        ping_message = {"command": "get_screen_size", "params": {}}

        # Try primary URI first (.sandbox.cua.ai)
        try:
            async with websockets.connect(
                self.ws_uri, max_size=1024 * 1024 * 10  # 10MB limit to match server
            ) as websocket:
                # Send ping message
                await websocket.send(json.dumps(ping_message))

                # Wait for any response or just close
                try:
                    response = await asyncio.wait_for(websocket.recv(), timeout=5)
                    logger.debug(f"Ping response received from primary URI: {response[:100]}...")
                    return True
                except asyncio.TimeoutError:
                    return False
        except Exception as e:
            logger.debug(f"Primary URI ping failed: {e}")

            # Try fallback URI (.containers.cloud.trycua.com)
            if self.container_name:
                try:
                    async with websockets.connect(
                        self.ws_uri_fallback,
                        max_size=1024 * 1024 * 10,  # 10MB limit to match server
                    ) as websocket:
                        # Send ping message
                        await websocket.send(json.dumps(ping_message))

                        # Wait for any response or just close
                        try:
                            response = await asyncio.wait_for(websocket.recv(), timeout=5)
                            logger.debug(
                                f"Ping response received from fallback URI: {response[:100]}..."
                            )
                            return True
                        except asyncio.TimeoutError:
                            return False
                except Exception as fallback_e:
                    logger.warning(
                        f"Both primary and fallback ping failed. Primary: {e}, Fallback: {fallback_e}"
                    )
                    return False
            else:
                logger.warning(f"Ping failed: {e}")
                return False

    def kill_processes_on_port(self, port: int) -> bool:
        """
        Kill any processes using the specified port.

        Args:
            port: Port number to check and kill processes on

        Returns:
            True if processes were killed or none found, False on error
        """
        try:
            # Find processes using the port
            result = subprocess.run(
                ["lsof", "-ti", f":{port}"], capture_output=True, text=True, timeout=10
            )

            if result.returncode == 0 and result.stdout.strip():
                pids = result.stdout.strip().split("\n")
                logger.info(f"Found {len(pids)} processes using port {port}: {pids}")

                # Kill each process
                for pid in pids:
                    if pid.strip():
                        try:
                            subprocess.run(["kill", "-9", pid.strip()], timeout=5)
                            logger.info(f"Killed process {pid}")
                        except subprocess.TimeoutExpired:
                            logger.warning(f"Timeout killing process {pid}")
                        except Exception as e:
                            logger.warning(f"Error killing process {pid}: {e}")

                return True
            else:
                logger.debug(f"No processes found using port {port}")
                return True

        except subprocess.TimeoutExpired:
            logger.error(f"Timeout finding processes on port {port}")
            return False
        except Exception as e:
            logger.error(f"Error finding processes on port {port}: {e}")
            return False

    def restart_server(self) -> bool:
        """
        Attempt to restart the server by killing existing processes and starting new one.

        Returns:
            True if restart was attempted, False on error
        """
        if not self.restart_enabled:
            logger.info("Server restart is disabled")
            return False

        try:
            logger.info("Attempting to restart server...")

            # Kill processes on the port
            port_to_kill = 8443 if self.container_name else self.port
            if not self.kill_processes_on_port(port_to_kill):
                logger.error("Failed to kill processes on port, restart aborted")
                return False

            # Wait a moment for processes to die
            time.sleep(2)

            # Try to restart the server
            # In container mode, we can't easily restart, so just log
            if self.container_name:
                logger.warning("Container mode detected - cannot restart server automatically")
                logger.warning("Container orchestrator should handle restart")
                return False
            else:
                # For local mode, try to restart the CLI
                logger.info("Attempting to restart local server...")

                # Get the current Python executable and script
                python_exe = sys.executable

                # Try to find the CLI module
                try:
                    # Build command with all original CLI arguments
                    cmd = [python_exe, "-m", "computer_server.cli"]

                    # Add all CLI arguments except watchdog-related ones
                    for key, value in self.cli_args.items():
                        if key in ["watchdog", "watchdog_interval", "no_restart"]:
                            continue  # Skip watchdog args to avoid recursive watchdog

                        # Convert underscores to hyphens for CLI args
                        arg_name = f"--{key.replace('_', '-')}"

                        if isinstance(value, bool):
                            if value:  # Only add flag if True
                                cmd.append(arg_name)
                        else:
                            cmd.extend([arg_name, str(value)])

                    logger.info(f"Starting server with command: {' '.join(cmd)}")

                    # Start process in background
                    subprocess.Popen(
                        cmd,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        start_new_session=True,
                    )

                    logger.info("Server restart initiated")
                    return True

                except Exception as e:
                    logger.error(f"Failed to restart server: {e}")
                    return False

        except Exception as e:
            logger.error(f"Error during server restart: {e}")
            return False

    async def start_monitoring(self) -> None:
        """Start the watchdog monitoring loop."""
        self.running = True
        logger.info(f"Starting watchdog monitoring for {self.ws_uri}")
        logger.info(f"Ping interval: {self.ping_interval} seconds")
        if self.container_name:
            logger.info(f"Container mode detected: {self.container_name}")

        consecutive_failures = 0
        max_failures = 3

        while self.running:
            try:
                success = await self.ping()

                if success:
                    if consecutive_failures > 0:
                        logger.info("Server connection restored")
                    consecutive_failures = 0
                    logger.debug("Ping successful")
                else:
                    consecutive_failures += 1
                    logger.warning(f"Ping failed ({consecutive_failures}/{max_failures})")

                    if consecutive_failures >= max_failures:
                        logger.error(
                            f"Server appears to be down after {max_failures} consecutive failures"
                        )

                        # Attempt to restart the server
                        if self.restart_enabled:
                            logger.info("Attempting automatic server restart...")
                            restart_success = self.restart_server()

                            if restart_success:
                                logger.info("Server restart initiated, waiting before next ping...")
                                # Wait longer after restart attempt
                                await asyncio.sleep(self.ping_interval * 2)
                                consecutive_failures = 0  # Reset counter after restart attempt
                            else:
                                logger.error("Server restart failed")
                        else:
                            logger.warning("Automatic restart is disabled")

                # Wait for next ping interval
                await asyncio.sleep(self.ping_interval)

            except asyncio.CancelledError:
                logger.info("Watchdog monitoring cancelled")
                break
            except Exception as e:
                logger.error(f"Unexpected error in watchdog loop: {e}")
                await asyncio.sleep(self.ping_interval)

    def stop_monitoring(self) -> None:
        """Stop the watchdog monitoring."""
        self.running = False
        logger.info("Stopping watchdog monitoring")


async def run_watchdog(cli_args: Optional[dict] = None, ping_interval: int = 30) -> None:
    """
    Run the watchdog monitoring.

    Args:
        cli_args: Dictionary of CLI arguments to replicate when restarting
        ping_interval: Interval between ping checks in seconds
    """
    watchdog = Watchdog(cli_args=cli_args, ping_interval=ping_interval)

    try:
        await watchdog.start_monitoring()
    except KeyboardInterrupt:
        logger.info("Watchdog stopped by user")
    finally:
        watchdog.stop_monitoring()


if __name__ == "__main__":
    # For testing the watchdog standalone
    import argparse

    parser = argparse.ArgumentParser(description="Run Computer API server watchdog")
    parser.add_argument("--host", default="localhost", help="Server host to monitor")
    parser.add_argument("--port", type=int, default=8000, help="Server port to monitor")
    parser.add_argument("--ping-interval", type=int, default=30, help="Ping interval in seconds")

    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    )

    cli_args = {"host": args.host, "port": args.port}
    asyncio.run(run_watchdog(cli_args, args.ping_interval))
