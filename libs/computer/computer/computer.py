from typing import Optional, List, Literal, Dict, Any, Union, TYPE_CHECKING, cast
from pylume import PyLume
from pylume.models import VMRunOpts, VMUpdateOpts, ImageRef, SharedDirectory, VMStatus
import asyncio
from .models import Computer as ComputerConfig, Display
from .interface.factory import InterfaceFactory
import time
from PIL import Image
import io
import re
from .logger import Logger, LogLevel
import json
import logging
from .telemetry import record_computer_initialization
import os

OSType = Literal["macos", "linux"]

# Import BaseComputerInterface for type annotations
if TYPE_CHECKING:
    from .interface.base import BaseComputerInterface


class Computer:
    """Computer is the main class for interacting with the computer."""

    def __init__(
        self,
        display: Union[Display, Dict[str, int], str] = "1024x768",
        memory: str = "8GB",
        cpu: str = "4",
        os_type: OSType = "macos",
        name: str = "",
        image: str = "macos-sequoia-cua:latest",
        shared_directories: Optional[List[str]] = None,
        use_host_computer_server: bool = False,
        verbosity: Union[int, LogLevel] = logging.INFO,
        telemetry_enabled: bool = True,
        port: Optional[int] = 3000,
        host: str = os.environ.get("PYLUME_HOST", "localhost"),
    ):
        """Initialize a new Computer instance.

        Args:
            display: The display configuration. Can be:
                    - A Display object
                    - A dict with 'width' and 'height'
                    - A string in format "WIDTHxHEIGHT" (e.g. "1920x1080")
                    Defaults to "1024x768"
            memory: The VM memory allocation. Defaults to "8GB"
            cpu: The VM CPU allocation. Defaults to "4"
            os: The operating system type ('macos' or 'linux')
            name: The VM name
            image: The VM image name
            shared_directories: Optional list of directory paths to share with the VM
            use_host_computer_server: If True, target localhost instead of starting a VM
            verbosity: Logging level (standard Python logging levels: logging.DEBUG, logging.INFO, etc.)
                      LogLevel enum values are still accepted for backward compatibility
            telemetry_enabled: Whether to enable telemetry tracking. Defaults to True.
            port: Optional port to use for the PyLume server
            host: Host to use for PyLume connections (e.g. "localhost", "host.docker.internal")
        """

        self.logger = Logger("cua.computer", verbosity)
        self.logger.info("Initializing Computer...")

        # Store original parameters
        self.image = image
        self.port = port
        self.host = host
        self.os_type = os_type

        # Store telemetry preference
        self._telemetry_enabled = telemetry_enabled

        # Set initialization flag
        self._initialized = False
        self._running = False

        # Configure root logger
        self.verbosity = verbosity
        self.logger = Logger("cua", verbosity)

        # Configure component loggers with proper hierarchy
        self.vm_logger = Logger("cua.vm", verbosity)
        self.interface_logger = Logger("cua.interface", verbosity)

        if not use_host_computer_server:
            if ":" not in image or len(image.split(":")) != 2:
                raise ValueError("Image must be in the format <image_name>:<tag>")

            if not name:
                # Normalize the name to be used for the VM
                name = image.replace(":", "_")

            # Convert display parameter to Display object
            if isinstance(display, str):
                # Parse string format "WIDTHxHEIGHT"
                match = re.match(r"(\d+)x(\d+)", display)
                if not match:
                    raise ValueError(
                        "Display string must be in format 'WIDTHxHEIGHT' (e.g. '1024x768')"
                    )
                width, height = map(int, match.groups())
                display_config = Display(width=width, height=height)
            elif isinstance(display, dict):
                display_config = Display(**display)
            else:
                display_config = display

            self.config = ComputerConfig(
                image=image.split(":")[0],
                tag=image.split(":")[1],
                name=name,
                display=display_config,
                memory=memory,
                cpu=cpu,
            )
            # Initialize PyLume but don't start the server yet - we'll do that in run()
            self.config.pylume = PyLume(
                debug=(self.verbosity == LogLevel.DEBUG),
                port=3000,
                use_existing_server=False,
                server_start_timeout=120,  # Increase timeout to 2 minutes
            )

        # Initialize with proper typing - None at first, will be set in run()
        self._interface = None
        self.os = os
        self.shared_paths = []
        if shared_directories:
            for path in shared_directories:
                abs_path = os.path.abspath(os.path.expanduser(path))
                if not os.path.exists(abs_path):
                    raise ValueError(f"Shared directory does not exist: {path}")
                self.shared_paths.append(abs_path)
        self._pylume_context = None
        self.use_host_computer_server = use_host_computer_server

        # Record initialization in telemetry (if enabled)
        if telemetry_enabled:
            record_computer_initialization()
        else:
            self.logger.debug("Telemetry disabled - skipping initialization tracking")

    async def __aenter__(self):
        """Enter async context manager."""
        await self.run()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Exit async context manager."""
        pass

    def __enter__(self):
        """Enter synchronous context manager."""
        # Run the event loop to call the async run method
        loop = asyncio.get_event_loop()
        loop.run_until_complete(self.run())
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Exit synchronous context manager."""
        # We could add cleanup here if needed in the future
        pass

    async def run(self) -> None:
        """Initialize the VM and computer interface."""
        if TYPE_CHECKING:
            from .interface.base import BaseComputerInterface

        # If already initialized, just log and return
        if hasattr(self, "_initialized") and self._initialized:
            self.logger.info("Computer already initialized, skipping initialization")
            return

        self.logger.info("Starting computer...")
        start_time = time.time()

        try:
            # If using host computer server
            if self.use_host_computer_server:
                self.logger.info("Using host computer server")
                # Set ip_address for host computer server mode
                ip_address = "localhost"
                # Create the interface with explicit type annotation
                from .interface.base import BaseComputerInterface

                self._interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type, ip_address=ip_address  # type: ignore[arg-type]
                    ),
                )

                self.logger.info("Waiting for host computer server to be ready...")
                await self._interface.wait_for_ready()
                self.logger.info("Host computer server ready")
            else:
                # Start or connect to VM
                self.logger.info(f"Starting VM: {self.image}")
                if not self._pylume_context:
                    try:
                        self.logger.verbose("Initializing PyLume context...")

                        # Configure PyLume based on initialization parameters
                        pylume_kwargs = {
                            "debug": self.verbosity <= LogLevel.DEBUG,
                            "server_start_timeout": 120,  # Increase timeout to 2 minutes
                        }

                        # Add port if specified
                        if hasattr(self, "port") and self.port is not None:
                            pylume_kwargs["port"] = self.port
                            self.logger.verbose(f"Using specified port for PyLume: {self.port}")

                        # Add host if specified
                        if hasattr(self, "host") and self.host != "localhost":
                            pylume_kwargs["host"] = self.host
                            self.logger.verbose(f"Using specified host for PyLume: {self.host}")

                        # Create PyLume instance with configured parameters
                        self.config.pylume = PyLume(**pylume_kwargs)

                        self._pylume_context = await self.config.pylume.__aenter__()  # type: ignore[attr-defined]
                        self.logger.verbose("PyLume context initialized successfully")
                    except Exception as e:
                        self.logger.error(f"Failed to initialize PyLume context: {e}")
                        raise RuntimeError(f"Failed to initialize PyLume: {e}")

                # Try to get the VM, if it doesn't exist, return an error
                try:
                    vm = await self.config.pylume.get_vm(self.config.name)  # type: ignore[attr-defined]
                    self.logger.verbose(f"Found existing VM: {self.config.name}")
                except Exception as e:
                    self.logger.error(f"VM not found: {self.config.name}")
                    self.logger.error(
                        f"Please pull the VM first with lume pull macos-sequoia-cua-sparse:latest: {e}"
                    )
                    raise RuntimeError(
                        f"VM not found: {self.config.name}. Please pull the VM first."
                    )

                # Convert paths to SharedDirectory objects
                shared_directories = []
                for path in self.shared_paths:
                    self.logger.verbose(f"Adding shared directory: {path}")
                    shared_directories.append(
                        SharedDirectory(host_path=path)  # type: ignore[arg-type]
                    )

                # Run with shared directories
                self.logger.info(f"Starting VM {self.config.name}...")
                run_opts = VMRunOpts(
                    no_display=False,  # type: ignore[arg-type]
                    shared_directories=shared_directories,  # type: ignore[arg-type]
                )

                # Log the run options for debugging
                self.logger.info(f"VM run options: {vars(run_opts)}")

                # Log the equivalent curl command for debugging
                payload = json.dumps({"noDisplay": False, "sharedDirectories": []})
                curl_cmd = f"curl -X POST 'http://localhost:3000/lume/vms/{self.config.name}/run' -H 'Content-Type: application/json' -d '{payload}'"
                self.logger.info(f"Equivalent curl command:")
                self.logger.info(f"{curl_cmd}")

                try:
                    response = await self.config.pylume.run_vm(self.config.name, run_opts)  # type: ignore[attr-defined]
                    self.logger.info(f"VM run response: {response if response else 'None'}")
                except Exception as run_error:
                    self.logger.error(f"Failed to run VM: {run_error}")
                    raise RuntimeError(f"Failed to start VM: {run_error}")

                # Wait for VM to be ready with required properties
                self.logger.info("Waiting for VM to be ready...")
                try:
                    vm = await self.wait_vm_ready()
                    if not vm or not vm.ip_address:  # type: ignore[attr-defined]
                        raise RuntimeError(f"VM {self.config.name} failed to get IP address")
                    ip_address = vm.ip_address  # type: ignore[attr-defined]
                    self.logger.info(f"VM is ready with IP: {ip_address}")
                except Exception as wait_error:
                    self.logger.error(f"Error waiting for VM: {wait_error}")
                    raise RuntimeError(f"VM failed to become ready: {wait_error}")
        except Exception as e:
            self.logger.error(f"Failed to initialize computer: {e}")
            raise RuntimeError(f"Failed to initialize computer: {e}")

        try:
            # Initialize the interface using the factory with the specified OS
            self.logger.info(f"Initializing interface for {self.os_type} at {ip_address}")
            from .interface.base import BaseComputerInterface

            self._interface = cast(
                BaseComputerInterface,
                InterfaceFactory.create_interface_for_os(
                    os=self.os_type, ip_address=ip_address  # type: ignore[arg-type]
                ),
            )

            # Wait for the WebSocket interface to be ready
            self.logger.info("Connecting to WebSocket interface...")

            try:
                # Use a single timeout for the entire connection process
                await self._interface.wait_for_ready(timeout=60)
                self.logger.info("WebSocket interface connected successfully")
            except TimeoutError as e:
                self.logger.error("Failed to connect to WebSocket interface")
                raise TimeoutError(
                    f"Could not connect to WebSocket interface at {ip_address}:8000/ws: {str(e)}"
                )

            # Create an event to keep the VM running in background if needed
            if not self.use_host_computer_server:
                self._stop_event = asyncio.Event()
                self._keep_alive_task = asyncio.create_task(self._stop_event.wait())

            self.logger.info("Computer is ready")

            # Set the initialization flag and clear the initializing flag
            self._initialized = True
            self.logger.info("Computer successfully initialized")
        except Exception as e:
            raise
        finally:
            # Log initialization time for performance monitoring
            duration_ms = (time.time() - start_time) * 1000
            self.logger.debug(f"Computer initialization took {duration_ms:.2f}ms")
        return

    async def stop(self) -> None:
        """Stop computer control."""
        start_time = time.time()

        try:
            if self._running:
                self._running = False
                self.logger.info("Stopping Computer...")

            if hasattr(self, "_stop_event"):
                self._stop_event.set()
                if hasattr(self, "_keep_alive_task"):
                    await self._keep_alive_task

            if self._interface:  # Only try to close interface if it exists
                self.logger.verbose("Closing interface...")
                # For host computer server, just use normal close to keep the server running
                if self.use_host_computer_server:
                    self._interface.close()
                else:
                    # For VM mode, force close the connection
                    if hasattr(self._interface, "force_close"):
                        self._interface.force_close()
                    else:
                        self._interface.close()

            if not self.use_host_computer_server and self._pylume_context:
                try:
                    self.logger.info(f"Stopping VM {self.config.name}...")
                    await self.config.pylume.stop_vm(self.config.name)  # type: ignore[attr-defined]
                except Exception as e:
                    self.logger.verbose(f"Error stopping VM: {e}")  # VM might already be stopped
                self.logger.verbose("Closing PyLume context...")
                await self.config.pylume.__aexit__(None, None, None)  # type: ignore[attr-defined]
                self._pylume_context = None
            self.logger.info("Computer stopped")
        except Exception as e:
            self.logger.debug(
                f"Error during cleanup: {e}"
            )  # Log as debug since this might be expected
        finally:
            # Log stop time for performance monitoring
            duration_ms = (time.time() - start_time) * 1000
            self.logger.debug(f"Computer stop process took {duration_ms:.2f}ms")
        return

    # @property
    async def get_ip(self) -> str:
        """Get the IP address of the VM or localhost if using host computer server."""
        if self.use_host_computer_server:
            return "127.0.0.1"
        ip = await self.config.get_ip()
        return ip or "unknown"  # Return "unknown" if ip is None

    async def wait_vm_ready(self) -> Optional[Union[Dict[str, Any], "VMStatus"]]:
        """Wait for VM to be ready with an IP address.

        Returns:
            VM status information or None if using host computer server.
        """
        if self.use_host_computer_server:
            return None

        timeout = 600  # 10 minutes timeout (increased from 4 minutes)
        interval = 2.0  # 2 seconds between checks (increased to reduce API load)
        start_time = time.time()
        last_status = None
        attempts = 0

        self.logger.info(f"Waiting for VM {self.config.name} to be ready (timeout: {timeout}s)...")

        while time.time() - start_time < timeout:
            attempts += 1
            elapsed = time.time() - start_time

            try:
                # Keep polling for VM info
                vm = await self.config.pylume.get_vm(self.config.name)  # type: ignore[attr-defined]

                # Log full VM properties for debugging (every 30 attempts)
                if attempts % 30 == 0:
                    self.logger.info(
                        f"VM properties at attempt {attempts}: {vars(vm) if vm else 'None'}"
                    )

                # Get current status for logging
                current_status = getattr(vm, "status", None) if vm else None
                if current_status != last_status:
                    self.logger.info(
                        f"VM status changed to: {current_status} (after {elapsed:.1f}s)"
                    )
                    last_status = current_status

                # Check for IP address - ensure it's not None or empty
                ip = getattr(vm, "ip_address", None) if vm else None
                if ip and ip.strip():  # Check for non-empty string
                    self.logger.info(
                        f"VM {self.config.name} got IP address: {ip} (after {elapsed:.1f}s)"
                    )
                    return vm

                if attempts % 10 == 0:  # Log every 10 attempts to avoid flooding
                    self.logger.info(
                        f"Still waiting for VM IP address... (elapsed: {elapsed:.1f}s)"
                    )
                else:
                    self.logger.debug(
                        f"Waiting for VM IP address... Current IP: {ip}, Status: {current_status}"
                    )

            except Exception as e:
                self.logger.warning(f"Error checking VM status (attempt {attempts}): {str(e)}")
                # If we've been trying for a while and still getting errors, log more details
                if elapsed > 60:  # After 1 minute of errors, log more details
                    self.logger.error(f"Persistent error getting VM status: {str(e)}")
                    self.logger.info("Trying to get VM list for debugging...")
                    try:
                        vms = await self.config.pylume.list_vms()  # type: ignore[attr-defined]
                        self.logger.info(
                            f"Available VMs: {[vm.name for vm in vms if hasattr(vm, 'name')]}"
                        )
                    except Exception as list_error:
                        self.logger.error(f"Failed to list VMs: {str(list_error)}")

            await asyncio.sleep(interval)

        # If we get here, we've timed out
        elapsed = time.time() - start_time
        self.logger.error(f"VM {self.config.name} not ready after {elapsed:.1f} seconds")

        # Try to get final VM status for debugging
        try:
            vm = await self.config.pylume.get_vm(self.config.name)  # type: ignore[attr-defined]
            status = getattr(vm, "status", "unknown") if vm else "unknown"
            ip = getattr(vm, "ip_address", None) if vm else None
            self.logger.error(f"Final VM status: {status}, IP: {ip}")
        except Exception as e:
            self.logger.error(f"Failed to get final VM status: {str(e)}")

        raise TimeoutError(
            f"VM {self.config.name} not ready after {elapsed:.1f} seconds - IP address not assigned"
        )

    async def update(self, cpu: Optional[int] = None, memory: Optional[str] = None):
        """Update VM settings."""
        self.logger.info(
            f"Updating VM settings: CPU={cpu or self.config.cpu}, Memory={memory or self.config.memory}"
        )
        update_opts = VMUpdateOpts(
            cpu=cpu or int(self.config.cpu), memory=memory or self.config.memory
        )
        await self.config.pylume.update_vm(self.config.image, update_opts)  # type: ignore[attr-defined]

    def get_screenshot_size(self, screenshot: bytes) -> Dict[str, int]:
        """Get the dimensions of a screenshot.

        Args:
            screenshot: The screenshot bytes

        Returns:
            Dict[str, int]: Dictionary containing 'width' and 'height' of the image
        """
        image = Image.open(io.BytesIO(screenshot))
        width, height = image.size
        return {"width": width, "height": height}

    @property
    def interface(self):
        """Get the computer interface for interacting with the VM.

        Returns:
            The computer interface
        """
        if not hasattr(self, "_interface") or self._interface is None:
            error_msg = "Computer interface not initialized. Call run() first."
            self.logger.error(error_msg)
            self.logger.error(
                "Make sure to call await computer.run() before using any interface methods."
            )
            raise RuntimeError(error_msg)

        return self._interface

    @property
    def telemetry_enabled(self) -> bool:
        """Check if telemetry is enabled for this computer instance.

        Returns:
            bool: True if telemetry is enabled, False otherwise
        """
        return self._telemetry_enabled

    async def to_screen_coordinates(self, x: float, y: float) -> tuple[float, float]:
        """Convert normalized coordinates to screen coordinates.

        Args:
            x: X coordinate between 0 and 1
            y: Y coordinate between 0 and 1

        Returns:
            tuple[float, float]: Screen coordinates (x, y)
        """
        return await self.interface.to_screen_coordinates(x, y)

    async def to_screenshot_coordinates(self, x: float, y: float) -> tuple[float, float]:
        """Convert screen coordinates to screenshot coordinates.

        Args:
            x: X coordinate in screen space
            y: Y coordinate in screen space

        Returns:
            tuple[float, float]: (x, y) coordinates in screenshot space
        """
        return await self.interface.to_screenshot_coordinates(x, y)
