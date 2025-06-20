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
from . import helpers

# Import provider related modules
from .providers.base import VMProviderType
from .providers.factory import VMProviderFactory

OSType = Literal["macos", "linux", "windows"]

class Computer:
    """Computer is the main class for interacting with the computer."""

    def create_desktop_from_apps(self, apps):
        """
        Create a virtual desktop from a list of app names, returning a DioramaComputer
        that proxies Diorama.Interface but uses diorama_cmds via the computer interface.

        Args:
            apps (list[str]): List of application names to include in the desktop.
        Returns:
            DioramaComputer: A proxy object with the Diorama interface, but using diorama_cmds.
        """
        assert "app-use" in self.experiments, "App Usage is an experimental feature. Enable it by passing experiments=['app-use'] to Computer()"
        from .diorama_computer import DioramaComputer
        return DioramaComputer(self, apps)

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
        port: Optional[int] = 7777,
        noVNC_port: Optional[int] = 8006,
        host: str = os.environ.get("PYLUME_HOST", "localhost"),
        storage: Optional[str] = None,
        ephemeral: bool = False,
        api_key: Optional[str] = None,
        experiments: Optional[List[str]] = None
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
            os_type: The operating system type ('macos' or 'linux')
            name: The VM name
            image: The VM image name
            shared_directories: Optional list of directory paths to share with the VM
            use_host_computer_server: If True, target localhost instead of starting a VM
            verbosity: Logging level (standard Python logging levels: logging.DEBUG, logging.INFO, etc.)
                      LogLevel enum values are still accepted for backward compatibility
            telemetry_enabled: Whether to enable telemetry tracking. Defaults to True.
            provider_type: The VM provider type to use (lume, qemu, cloud)
            port: Optional port to use for the VM provider server
            noVNC_port: Optional port for the noVNC web interface (Lumier provider)
            host: Host to use for VM provider connections (e.g. "localhost", "host.docker.internal")
            storage: Optional path for persistent VM storage (Lumier provider)
            ephemeral: Whether to use ephemeral storage
            api_key: Optional API key for cloud providers
            experiments: Optional list of experimental features to enable (e.g. ["app-use"])
        """

        self.logger = Logger("computer", verbosity)
        self.logger.info("Initializing Computer...")

        # Store original parameters
        self.image = image
        self.port = port
        self.noVNC_port = noVNC_port
        self.host = host
        self.os_type = os_type
        self.provider_type = provider_type
        self.ephemeral = ephemeral
        
        self.api_key = api_key
        self.experiments = experiments or []
        
        if "app-use" in self.experiments:
            assert self.os_type == "macos", "App use experiment is only supported on macOS"

        # The default is currently to use non-ephemeral storage
        if storage and ephemeral and storage != "ephemeral":
            raise ValueError("Storage path and ephemeral flag cannot be used together")
        
        # Windows Sandbox always uses ephemeral storage
        if self.provider_type == VMProviderType.WINSANDBOX:
            if not ephemeral and storage != None and storage != "ephemeral":
                self.logger.warning("Windows Sandbox storage is always ephemeral. Setting ephemeral=True.")
            self.ephemeral = True
            self.storage = "ephemeral"
        else:
            self.storage = "ephemeral" if ephemeral else storage
        
        # For Lumier provider, store the first shared directory path to use
        # for VM file sharing
        self.shared_path = None
        if shared_directories and len(shared_directories) > 0:
            self.shared_path = shared_directories[0]
            self.logger.info(f"Using first shared directory for VM file sharing: {self.shared_path}")

        # Store telemetry preference
        self._telemetry_enabled = telemetry_enabled

        # Set initialization flag
        self._initialized = False
        self._running = False

        # Configure root logger
        self.verbosity = verbosity
        self.logger = Logger("computer", verbosity)

        # Configure component loggers with proper hierarchy
        self.vm_logger = Logger("computer.vm", verbosity)
        self.interface_logger = Logger("computer.interface", verbosity)

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
        """Start the computer."""
        await self.run()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Stop the computer."""
        await self.disconnect()

    def __enter__(self):
        """Start the computer."""
        # Run the event loop to call the async enter method
        loop = asyncio.get_event_loop()
        loop.run_until_complete(self.__aenter__())
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Stop the computer."""
        loop = asyncio.get_event_loop()
        loop.run_until_complete(self.__aexit__(exc_type, exc_val, exc_tb))

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

                        # Explicitly set provider parameters
                        storage = "ephemeral" if self.ephemeral else self.storage
                        verbose = self.verbosity >= LogLevel.DEBUG
                        ephemeral = self.ephemeral
                        port = self.port if self.port is not None else 7777
                        host = self.host if self.host else "localhost"
                        image = self.image
                        shared_path = self.shared_path
                        noVNC_port = self.noVNC_port

                        # Create VM provider instance with explicit parameters
                        try:
                            if self.provider_type == VMProviderType.LUMIER:
                                self.logger.info(f"Using VM image for Lumier provider: {image}")
                                if shared_path:
                                    self.logger.info(f"Using shared path for Lumier provider: {shared_path}")
                                if noVNC_port:
                                    self.logger.info(f"Using noVNC port for Lumier provider: {noVNC_port}")
                                self.config.vm_provider = VMProviderFactory.create_provider(
                                    self.provider_type,
                                    port=port,
                                    host=host,
                                    storage=storage,
                                    shared_path=shared_path,
                                    image=image,
                                    verbose=verbose,
                                    ephemeral=ephemeral,
                                    noVNC_port=noVNC_port,
                                )
                            elif self.provider_type == VMProviderType.LUME:
                                self.config.vm_provider = VMProviderFactory.create_provider(
                                    self.provider_type,
                                    port=port,
                                    host=host,
                                    storage=storage,
                                    verbose=verbose,
                                    ephemeral=ephemeral,
                                )
                            elif self.provider_type == VMProviderType.CLOUD:
                                self.config.vm_provider = VMProviderFactory.create_provider(
                                    self.provider_type,
                                    api_key=self.api_key,
                                    verbose=verbose,
                                )
                            elif self.provider_type == VMProviderType.WINSANDBOX:
                                self.config.vm_provider = VMProviderFactory.create_provider(
                                    self.provider_type,
                                    port=port,
                                    host=host,
                                    storage=storage,
                                    verbose=verbose,
                                    ephemeral=ephemeral,
                                )
                            else:
                                raise ValueError(f"Unsupported provider type: {self.provider_type}")
                            self._provider_context = await self.config.vm_provider.__aenter__()
                            self.logger.verbose("VM provider context initialized successfully")
                        except ImportError as ie:
                            self.logger.error(f"Failed to import provider dependencies: {ie}")
                            if str(ie).find("lume") >= 0 and str(ie).find("lumier") < 0:
                                self.logger.error("Please install with: pip install cua-computer[lume]")
                            elif str(ie).find("lumier") >= 0 or str(ie).find("docker") >= 0:
                                self.logger.error("Please install with: pip install cua-computer[lumier] and make sure Docker is installed")
                            elif str(ie).find("cloud") >= 0:
                                self.logger.error("Please install with: pip install cua-computer[cloud]")
                            raise
                    except Exception as e:
                        self.logger.error(f"Failed to initialize provider context: {e}")
                        raise RuntimeError(f"Failed to initialize VM provider: {e}")

                # Check if VM exists or create it
                is_running = False
                try:
                    if self.config.vm_provider is None:
                        raise RuntimeError(f"VM provider not initialized for {self.config.name}")
                        
                    vm = await self.config.vm_provider.get_vm(self.config.name)
                    self.logger.verbose(f"Found existing VM: {self.config.name}")
                    is_running = vm.get("status") == "running"
                except Exception as e:
                    self.logger.error(f"VM not found: {self.config.name}")
                    self.logger.error(f"Error: {e}")
                    raise RuntimeError(
                        f"VM {self.config.name} could not be found or created."
                    )

                # Start the VM if it's not running
                if not is_running:
                    self.logger.info(f"VM {self.config.name} is not running, starting it...")

                    # Convert paths to dictionary format for shared directories
                    shared_dirs = []
                    for path in self.shared_directories:
                        self.logger.verbose(f"Adding shared directory: {path}")
                        path = os.path.abspath(os.path.expanduser(path))
                        if os.path.exists(path):
                            # Add path in format expected by Lume API
                            shared_dirs.append({
                                "hostPath": path,
                                "readOnly": False
                            })
                        else:
                            self.logger.warning(f"Shared directory does not exist: {path}")
                            
                    # Prepare run options to pass to the provider
                    run_opts = {}

                    # Add display information if available
                    if self.config.display is not None:
                        display_info = {
                            "width": self.config.display.width,
                            "height": self.config.display.height,
                        }
                        
                        # Check if scale_factor exists before adding it
                        if hasattr(self.config.display, "scale_factor"):
                            display_info["scale_factor"] = self.config.display.scale_factor
                        
                        run_opts["display"] = display_info

                    # Add shared directories if available
                    if self.shared_directories:
                        run_opts["shared_directories"] = shared_dirs.copy()

                    # Run the VM with the provider
                    try:
                        if self.config.vm_provider is None:
                            raise RuntimeError(f"VM provider not initialized for {self.config.name}")
                            
                        # Use the complete run_opts we prepared earlier
                        # Handle ephemeral storage for run_vm method too
                        storage_param = "ephemeral" if self.ephemeral else self.storage
                        
                        # Log the image being used
                        self.logger.info(f"Running VM using image: {self.image}")
                        
                        # Call provider.run_vm with explicit image parameter
                        response = await self.config.vm_provider.run_vm(
                            image=self.image,
                            name=self.config.name,
                            run_opts=run_opts,
                            storage=storage_param
                        )
                        self.logger.info(f"VM run response: {response if response else 'None'}")
                    except Exception as run_error:
                        self.logger.error(f"Failed to run VM: {run_error}")
                        raise RuntimeError(f"Failed to start VM: {run_error}")

                # Wait for VM to be ready with a valid IP address
                self.logger.info("Waiting for VM to be ready with a valid IP address...")
                try:
                    if self.provider_type == VMProviderType.LUMIER:
                        max_retries = 60  # Increased for Lumier VM startup which takes longer
                        retry_delay = 3    # 3 seconds between retries for Lumier
                    else:
                        max_retries = 30  # Default for other providers
                        retry_delay = 2    # 2 seconds between retries
                    
                    self.logger.info(f"Waiting up to {max_retries * retry_delay} seconds for VM to be ready...")
                    ip = await self.get_ip(max_retries=max_retries, retry_delay=retry_delay)
                    
                    # If we get here, we have a valid IP
                    self.logger.info(f"VM is ready with IP: {ip}")
                    ip_address = ip
                except TimeoutError as timeout_error:
                    self.logger.error(str(timeout_error))
                    raise RuntimeError(f"VM startup timed out: {timeout_error}")
                except Exception as wait_error:
                    self.logger.error(f"Error waiting for VM: {wait_error}")
                    raise RuntimeError(f"VM failed to become ready: {wait_error}")
        except Exception as e:
            self.logger.error(f"Failed to initialize computer: {e}")
            raise RuntimeError(f"Failed to initialize computer: {e}")

        try:
            # Verify we have a valid IP before initializing the interface
            if not ip_address or ip_address == "unknown" or ip_address == "0.0.0.0":
                raise RuntimeError(f"Cannot initialize interface - invalid IP address: {ip_address}")
                
            # Initialize the interface using the factory with the specified OS
            self.logger.info(f"Initializing interface for {self.os_type} at {ip_address}")
            from .interface.base import BaseComputerInterface

            # Pass authentication credentials if using cloud provider
            if self.provider_type == VMProviderType.CLOUD and self.api_key and self.config.name:
                self._interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type, 
                        ip_address=ip_address,
                        api_key=self.api_key,
                        vm_name=self.config.name
                    ),
                )
            else:
                self._interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type, 
                        ip_address=ip_address
                    ),
                )

            # Wait for the WebSocket interface to be ready
            self.logger.info("Connecting to WebSocket interface...")

            try:
                # Use a single timeout for the entire connection process
                # The VM should already be ready at this point, so we're just establishing the connection
                await self._interface.wait_for_ready(timeout=30)
                self.logger.info("WebSocket interface connected successfully")
            except TimeoutError as e:
                self.logger.error(f"Failed to connect to WebSocket interface at {ip_address}")
                raise TimeoutError(
                    f"Could not connect to WebSocket interface at {ip_address}:8000/ws: {str(e)}"
                )
                # self.logger.warning(
                #     f"Could not connect to WebSocket interface at {ip_address}:8000/ws: {str(e)}, expect missing functionality"
                # )

            # Create an event to keep the VM running in background if needed
            if not self.use_host_computer_server:
                self._stop_event = asyncio.Event()
                self._keep_alive_task = asyncio.create_task(self._stop_event.wait())

            self.logger.info("Computer is ready")

            # Set the initialization flag and clear the initializing flag
            self._initialized = True
            
            # Set this instance as the default computer for remote decorators
            helpers.set_default_computer(self)
            
            self.logger.info("Computer successfully initialized")
        except Exception as e:
            raise
        finally:
            # Log initialization time for performance monitoring
            duration_ms = (time.time() - start_time) * 1000
            self.logger.debug(f"Computer initialization took {duration_ms:.2f}ms")
        return
    
    async def disconnect(self) -> None:
        """Disconnect from the computer's WebSocket interface."""
        if self._interface:
            self._interface.close()

    async def stop(self) -> None:
        """Disconnect from the computer's WebSocket interface and stop the computer."""
        start_time = time.time()

        try:
            self.logger.info("Stopping Computer...")

            # In VM mode, first explicitly stop the VM, then exit the provider context
            if not self.use_host_computer_server and self._provider_context and self.config.vm_provider is not None:
                try:
                    self.logger.info(f"Stopping VM {self.config.name}...")
                    await self.config.vm_provider.stop_vm(
                    name=self.config.name,
                    storage=self.storage  # Pass storage explicitly for clarity
                )
                except Exception as e:
                    self.logger.error(f"Error stopping VM: {e}")

                self.logger.verbose("Closing VM provider context...")
                await self.config.vm_provider.__aexit__(None, None, None)
                self._provider_context = None

            await self.disconnect()
            self.logger.info("Computer stopped")
        except Exception as e:
            self.logger.debug(f"Error during cleanup: {e}")  # Log as debug since this might be expected
        finally:
            # Log stop time for performance monitoring
            duration_ms = (time.time() - start_time) * 1000
            self.logger.debug(f"Computer stop process took {duration_ms:.2f}ms")
        return

    # @property
    async def get_ip(self, max_retries: int = 15, retry_delay: int = 3) -> str:
        """Get the IP address of the VM or localhost if using host computer server.
        
        This method delegates to the provider's get_ip method, which waits indefinitely 
        until the VM has a valid IP address.
        
        Args:
            max_retries: Unused parameter, kept for backward compatibility
            retry_delay: Delay between retries in seconds (default: 2)
            
        Returns:
            IP address of the VM or localhost if using host computer server
        """
        # For host computer server, always return localhost immediately
        if self.use_host_computer_server:
            return "127.0.0.1"
            
        # Get IP from the provider - each provider implements its own waiting logic
        if self.config.vm_provider is None:
            raise RuntimeError("VM provider is not initialized")
        
        # Log that we're waiting for the IP
        self.logger.info(f"Waiting for VM {self.config.name} to get an IP address...")
        
        # Call the provider's get_ip method which will wait indefinitely
        storage_param = "ephemeral" if self.ephemeral else self.storage
        
        # Log the image being used
        self.logger.info(f"Running VM using image: {self.image}")
        
        # Call provider.get_ip with explicit image parameter
        ip = await self.config.vm_provider.get_ip(
            name=self.config.name,
            storage=storage_param,
            retry_delay=retry_delay
        )
        
        # Log success
        self.logger.info(f"VM {self.config.name} has IP address: {ip}")
        return ip
        

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
                if self.config.vm_provider is None:
                    self.logger.error("VM provider is not initialized")
                    vm = None
                else:
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
                # VM data is returned as a dictionary from the Lumier provider
                status = vm.get('status', 'unknown') if vm else "unknown"
                ip = vm.get('ip_address') if vm else None
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
                await self.config.vm_provider.update_vm(
                    name=self.config.name,
                    update_opts=update_opts,
                    storage=self.storage  # Pass storage explicitly for clarity
                )
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


    # Add virtual environment management functions to computer interface
    async def venv_install(self, venv_name: str, requirements: list[str]) -> tuple[str, str]:
        """Install packages in a virtual environment.
        
        Args:
            venv_name: Name of the virtual environment
            requirements: List of package requirements to install
            
        Returns:
            Tuple of (stdout, stderr) from the installation command
        """
        requirements = requirements or []

        # Create virtual environment if it doesn't exist
        venv_path = f"~/.venvs/{venv_name}"
        create_cmd = f"mkdir -p ~/.venvs && python3 -m venv {venv_path}"
        
        # Check if venv exists, if not create it
        check_cmd = f"test -d {venv_path} || ({create_cmd})"
        _, _ = await self.interface.run_command(check_cmd)
        
        # Install packages
        requirements_str = " ".join(requirements)
        install_cmd = f". {venv_path}/bin/activate && pip install {requirements_str}"
        return await self.interface.run_command(install_cmd)
    
    async def venv_cmd(self, venv_name: str, command: str) -> tuple[str, str]:
        """Execute a shell command in a virtual environment.
        
        Args:
            venv_name: Name of the virtual environment
            command: Shell command to execute in the virtual environment
            
        Returns:
            Tuple of (stdout, stderr) from the command execution
        """
        venv_path = f"~/.venvs/{venv_name}"
        
        # Check if virtual environment exists
        check_cmd = f"test -d {venv_path}"
        stdout, stderr = await self.interface.run_command(check_cmd)
        
        if stderr or "test:" in stdout:  # venv doesn't exist
            return "", f"Virtual environment '{venv_name}' does not exist. Create it first using venv_install."
        
        # Activate virtual environment and run command
        full_command = f". {venv_path}/bin/activate && {command}"
        return await self.interface.run_command(full_command)
    
    async def venv_exec(self, venv_name: str, python_func, *args, **kwargs):
        """Execute Python function in a virtual environment using source code extraction.
        
        Args:
            venv_name: Name of the virtual environment
            python_func: A callable function to execute
            *args: Positional arguments to pass to the function
            **kwargs: Keyword arguments to pass to the function
            
        Returns:
            The result of the function execution, or raises any exception that occurred
        """
        import base64
        import inspect
        import json
        import textwrap
        
        try:
            # Get function source code using inspect.getsource
            source = inspect.getsource(python_func)
            # Remove common leading whitespace (dedent)
            func_source = textwrap.dedent(source).strip()
            
            # Remove decorators
            while func_source.lstrip().startswith("@"):
                func_source = func_source.split("\n", 1)[1].strip()
            
            # Get function name for execution
            func_name = python_func.__name__
            
            # Serialize args and kwargs as JSON (safer than dill for cross-version compatibility)
            args_json = json.dumps(args, default=str)
            kwargs_json = json.dumps(kwargs, default=str)
            
        except OSError as e:
            raise Exception(f"Cannot retrieve source code for function {python_func.__name__}: {e}")
        except Exception as e:
            raise Exception(f"Failed to reconstruct function source: {e}")
        
        # Create Python code that will define and execute the function
        python_code = f'''
import json
import traceback

try:
    # Define the function from source
{textwrap.indent(func_source, "    ")}
    
    # Deserialize args and kwargs from JSON
    args_json = """{args_json}"""
    kwargs_json = """{kwargs_json}"""
    args = json.loads(args_json)
    kwargs = json.loads(kwargs_json)
    
    # Execute the function
    result = {func_name}(*args, **kwargs)

    # Create success output payload
    output_payload = {{
        "success": True,
        "result": result,
        "error": None
    }}
    
except Exception as e:
    # Create error output payload
    output_payload = {{
        "success": False,
        "result": None,
        "error": {{
            "type": type(e).__name__,
            "message": str(e),
            "traceback": traceback.format_exc()
        }}
    }}

# Serialize the output payload as JSON
import json
output_json = json.dumps(output_payload, default=str)

# Print the JSON output with markers
print(f"<<<VENV_EXEC_START>>>{{output_json}}<<<VENV_EXEC_END>>>")
'''
        
        # Encode the Python code in base64 to avoid shell escaping issues
        encoded_code = base64.b64encode(python_code.encode('utf-8')).decode('ascii')
        
        # Execute the Python code in the virtual environment
        python_command = f"python -c \"import base64; exec(base64.b64decode('{encoded_code}').decode('utf-8'))\""
        stdout, stderr = await self.venv_cmd(venv_name, python_command)
        
        # Parse the output to extract the payload
        start_marker = "<<<VENV_EXEC_START>>>"
        end_marker = "<<<VENV_EXEC_END>>>"

        # Print original stdout
        print(stdout[:stdout.find(start_marker)])
        
        if start_marker in stdout and end_marker in stdout:
            start_idx = stdout.find(start_marker) + len(start_marker)
            end_idx = stdout.find(end_marker)
            
            if start_idx < end_idx:
                output_json = stdout[start_idx:end_idx]

                try:
                    # Decode and deserialize the output payload from JSON
                    output_payload = json.loads(output_json)
                except Exception as e:
                    raise Exception(f"Failed to decode output payload: {e}")
                
                if output_payload["success"]:
                    return output_payload["result"]
                else:
                    # Recreate and raise the original exception
                    error_info = output_payload["error"]
                    error_class = eval(error_info["type"])
                    raise error_class(error_info["message"])
            else:
                raise Exception("Invalid output format: markers found but no content between them")
        else:
            # Fallback: return stdout/stderr if no payload markers found
            raise Exception(f"No output payload found. stdout: {stdout}, stderr: {stderr}")
