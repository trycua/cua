from typing import Optional, List, Literal, Dict, Any, Union, TYPE_CHECKING, cast
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

# Import provider related modules
from .providers.base import VMProviderType
from .providers.factory import VMProviderFactory

OSType = Literal["macos", "linux"]

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
        provider_type: Union[str, VMProviderType] = VMProviderType.LUME,
        port: Optional[int] = 3000,
        host: str = os.environ.get("PYLUME_HOST", "localhost"),
        storage_path: Optional[str] = None,
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
            provider_type: The VM provider type to use (lume, qemu, cloud)
            port: Optional port to use for the VM provider server
            host: Host to use for VM provider connections (e.g. "localhost", "host.docker.internal")
            bin_path: Optional path to the VM provider binary
            storage_path: Optional path to store VM data
        """

        self.logger = Logger("cua.computer", verbosity)
        self.logger.info("Initializing Computer...")

        # Store original parameters
        self.image = image
        self.port = port
        self.host = host
        self.os_type = os_type
        self.provider_type = provider_type
        self.storage_path = storage_path

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
            # Initialize VM provider but don't start it yet - we'll do that in run()
            self.config.vm_provider = None  # Will be initialized in run()

        # Store shared directories config
        self.shared_directories = shared_directories or []

        # Placeholder for VM provider context manager
        self._provider_context = None

        # Initialize with proper typing - None at first, will be set in run()
        self._interface = None
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

    async def run(self) -> Optional[str]:
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
                if not self._provider_context:
                    try:
                        provider_type_name = self.provider_type.name if isinstance(self.provider_type, VMProviderType) else self.provider_type
                        self.logger.verbose(f"Initializing {provider_type_name} provider context...")

                        # Configure provider based on initialization parameters
                        provider_kwargs = {
                            "storage_path": self.storage_path,
                            "verbose": self.verbosity >= LogLevel.DEBUG,
                        }

                        # Set port if specified
                        if self.port is not None:
                            provider_kwargs["port"] = self.port
                            self.logger.verbose(f"Using specified port for provider: {self.port}")

                        # Set host if specified 
                        if self.host:
                            provider_kwargs["host"] = self.host
                            self.logger.verbose(f"Using specified host for provider: {self.host}")

                        # Create VM provider instance with configured parameters
                        try:
                            self.config.vm_provider = VMProviderFactory.create_provider(
                                self.provider_type, **provider_kwargs
                            )
                            self._provider_context = await self.config.vm_provider.__aenter__()
                            self.logger.verbose("VM provider context initialized successfully")
                        except ImportError as ie:
                            self.logger.error(f"Failed to import provider dependencies: {ie}")
                            if str(ie).find("lume") >= 0:
                                self.logger.error("Please install with: pip install cua-computer[lume]")
                            elif str(ie).find("qemu") >= 0:
                                self.logger.error("Please install with: pip install cua-computer[qemu]")
                            elif str(ie).find("cloud") >= 0:
                                self.logger.error("Please install with: pip install cua-computer[cloud]")
                            raise
                    except Exception as e:
                        self.logger.error(f"Failed to initialize provider context: {e}")
                        raise RuntimeError(f"Failed to initialize VM provider: {e}")

                # Check if VM exists or create it
                try:
                    if self.config.vm_provider is None:
                        raise RuntimeError(f"VM provider not initialized for {self.config.name}")
                        
                    vm = await self.config.vm_provider.get_vm(self.config.name)
                    self.logger.verbose(f"Found existing VM: {self.config.name}")
                except Exception as e:
                    self.logger.error(f"VM not found: {self.config.name}")
                    self.logger.error(f"Error: {e}")
                    raise RuntimeError(
                        f"VM {self.config.name} could not be found or created."
                    )

                # Convert paths to dictionary format for shared directories
                shared_dirs = []
                for path in self.shared_directories:
                    self.logger.verbose(f"Adding shared directory: {path}")
                    path = os.path.abspath(os.path.expanduser(path))
                    if not os.path.exists(path):
                        self.logger.warning(f"Shared directory does not exist: {path}")
                        continue
                    shared_dirs.append({"host_path": path, "vm_path": path})

                # Create VM run options with specs from config
                # Account for optional shared directories
                run_opts = {
                    "cpu": int(self.config.cpu),
                    "memory": self.config.memory,
                    "display": {
                        "width": self.config.display.width, 
                        "height": self.config.display.height
                    }
                }
                
                if shared_dirs:
                    run_opts["shared_directories"] = shared_dirs

                # Log the run options for debugging
                self.logger.info(f"VM run options: {run_opts}")

                # Log the equivalent curl command for debugging
                payload = json.dumps({"noDisplay": False, "sharedDirectories": []})
                curl_cmd = f"curl -X POST 'http://localhost:3000/lume/vms/{self.config.name}/run' -H 'Content-Type: application/json' -d '{payload}'"
                # self.logger.info(f"Equivalent curl command:")
                # self.logger.info(f"{curl_cmd}")

                try:
                    if self.config.vm_provider is None:
                        raise RuntimeError(f"VM provider not initialized for {self.config.name}")
                        
                    response = await self.config.vm_provider.run_vm(self.config.name, run_opts)
                    self.logger.info(f"VM run response: {response if response else 'None'}")
                except Exception as run_error:
                    self.logger.error(f"Failed to run VM: {run_error}")
                    raise RuntimeError(f"Failed to start VM: {run_error}")

                # Wait for VM to be ready with required properties
                self.logger.info("Waiting for VM to be ready...")
                try:
                    ip = await self.get_ip()
                    if ip:
                        self.logger.info(f"VM is ready with IP: {ip}")
                        # Store the IP address for later use instead of returning early
                        ip_address = ip
                    else:
                        # If no IP was found, try to raise a helpful error
                        raise RuntimeError(f"VM {self.config.name} failed to get IP address")
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

            if not self.use_host_computer_server and self._provider_context:
                try:
                    self.logger.info(f"Stopping VM {self.config.name}...")
                    if self.config.vm_provider is not None:
                        await self.config.vm_provider.stop_vm(self.config.name)
                except Exception as e:
                    self.logger.error(f"Error stopping VM: {e}")

                self.logger.verbose("Closing VM provider context...")
                if self.config.vm_provider is not None:
                    await self.config.vm_provider.__aexit__(None, None, None)
                self._provider_context = None
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

    async def wait_vm_ready(self) -> Optional[Dict[str, Any]]:
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
                vm = await self.config.vm_provider.get_vm(self.config.name)

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
                        if self.config.vm_provider is not None:
                            vms = await self.config.vm_provider.list_vms()
                            self.logger.info(
                                f"Available VMs: {[getattr(vm, 'name', None) for vm in vms if hasattr(vm, 'name')]}"
                            )
                    except Exception as list_error:
                        self.logger.error(f"Failed to list VMs: {str(list_error)}")

            await asyncio.sleep(interval)

        # If we get here, we've timed out
        elapsed = time.time() - start_time
        self.logger.error(f"VM {self.config.name} not ready after {elapsed:.1f} seconds")

        # Try to get final VM status for debugging
        try:
            if self.config.vm_provider is not None:
                vm = await self.config.vm_provider.get_vm(self.config.name)
                # VMStatus is a Pydantic model with attributes, not a dictionary
                status = vm.status if vm else "unknown"
                ip = vm.ip_address if vm else None
            else:
                status = "unknown"
                ip = None
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
        update_opts = {
            "cpu": cpu or int(self.config.cpu), 
            "memory": memory or self.config.memory
        }
        if self.config.vm_provider is not None:
            await self.config.vm_provider.update_vm(self.config.name, update_opts)
        else:
            raise RuntimeError("VM provider not initialized")

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
