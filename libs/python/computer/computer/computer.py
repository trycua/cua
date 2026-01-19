import asyncio
import io
import json
import logging
import os
import platform
import re
import time
import traceback
from functools import wraps
from typing import (
    TYPE_CHECKING,
    Any,
    Awaitable,
    Callable,
    Dict,
    List,
    Literal,
    Optional,
    TypeVar,
    Union,
    cast,
)

try:
    from typing import ParamSpec
except Exception:  # pragma: no cover
    from typing_extensions import ParamSpec  # type: ignore

P = ParamSpec("P")
R = TypeVar("R")

from core.telemetry import is_telemetry_enabled, record_event
from PIL import Image

from . import helpers
from .interface.factory import InterfaceFactory
from .logger import Logger, LogLevel
from .models import Computer as ComputerConfig
from .models import Display
from .otel_wrapper import OtelInterfaceWrapper, wrap_interface_with_otel
from .tracing import ComputerTracing
from .tracing_wrapper import TracingInterfaceWrapper

# Import OTEL functions for session-level metrics
try:
    from core.telemetry import (
        add_breadcrumb,
        capture_exception,
        is_otel_enabled,
        record_operation,
        track_concurrent,
    )

    OTEL_AVAILABLE = True
except ImportError:
    OTEL_AVAILABLE = False

    def is_otel_enabled() -> bool:
        return False

SYSTEM_INFO = {
    "os": platform.system().lower(),
    "os_version": platform.release(),
    "python_version": platform.python_version(),
}

# Import provider related modules
from .providers.base import VMProviderType
from .providers.factory import VMProviderFactory

OSType = Literal["macos", "linux", "windows", "android"]


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
        assert (
            "app-use" in self.experiments
        ), "App Usage is an experimental feature. Enable it by passing experiments=['app-use'] to Computer()"
        from .diorama_computer import DioramaComputer

        return DioramaComputer(self, apps)

    def __init__(
        self,
        display: Union[Display, Dict[str, int], str] = "1024x768",
        memory: str = "8GB",
        cpu: str = "4",
        os_type: OSType = "macos",
        name: str = "",
        image: Optional[str] = None,
        shared_directories: Optional[List[str]] = None,
        use_host_computer_server: bool = False,
        verbosity: Union[int, LogLevel] = logging.INFO,
        telemetry_enabled: bool = True,
        provider_type: Union[str, VMProviderType] = VMProviderType.LUME,
        provider_port: Optional[int] = 7777,
        noVNC_port: Optional[int] = 8006,
        api_port: Optional[int] = None,
        host: str = "localhost",
        api_host: Optional[str] = None,
        storage: Optional[str] = None,
        ephemeral: bool = False,
        api_key: Optional[str] = None,
        experiments: Optional[List[str]] = None,
        timeout: int = 100,
        run_opts: Optional[Dict[str, Any]] = None,
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
            provider_port: Optional port to use for the VM provider server
            noVNC_port: Optional port for the noVNC web interface (Lumier provider)
            api_port: Optional port for the computer API server
            host: Host to use for VM provider connections (e.g. "localhost", "host.docker.internal")
            api_host: Optional host IP address to use when use_host_computer_server is True (defaults to "localhost")
            storage: Optional path for persistent VM storage (Lumier provider)
            ephemeral: Whether to use ephemeral storage
            api_key: Optional API key for cloud providers (defaults to CUA_API_KEY environment variable)
            experiments: Optional list of experimental features to enable (e.g. ["app-use"])
            timeout: Timeout in seconds for connecting to the computer interface
            run_opts: Optional dictionary of provider-specific run options.
        """

        self.logger = Logger("computer", verbosity)
        self.logger.info("Initializing Computer...")

        # Fall back to environment variable for api_key if not provided
        if api_key is None:
            api_key = os.environ.get("CUA_API_KEY")

        if not image:
            if os_type == "macos":
                image = "macos-sequoia-cua:latest"
            elif os_type == "linux":
                image = "trycua/cua-ubuntu:latest"
        image = str(image)

        # Store original parameters
        self.image = image
        self.host = host
        self.provider_port = provider_port
        self.noVNC_port = noVNC_port
        self.api_port = api_port
        self.api_host = api_host
        self.os_type = os_type
        self.provider_type = provider_type
        self.ephemeral = ephemeral
        self.api_key = api_key if self.provider_type == VMProviderType.CLOUD else None
        self.timeout = timeout

        # Set default API port if not specified
        if self.api_port is None:
            self.api_port = 8443 if self.api_key else 8000

        self.experiments = experiments or []

        if "app-use" in self.experiments:
            assert self.os_type == "macos", "App use experiment is only supported on macOS"

        # Store custom run options
        self.custom_run_opts = run_opts or {}

        # The default is currently to use non-ephemeral storage
        if storage and ephemeral and storage != "ephemeral":
            raise ValueError("Storage path and ephemeral flag cannot be used together")

        # Windows Sandbox always uses ephemeral storage
        if self.provider_type == VMProviderType.WINSANDBOX:
            if not ephemeral and storage != None and storage != "ephemeral":
                self.logger.warning(
                    "Windows Sandbox storage is always ephemeral. Setting ephemeral=True."
                )
            self.ephemeral = True
            self.storage = "ephemeral"
        else:
            self.storage = "ephemeral" if ephemeral else storage

        # For Lumier provider, store the first shared directory path to use
        # for VM file sharing
        self.shared_path = None
        if shared_directories and len(shared_directories) > 0:
            self.shared_path = shared_directories[0]
            self.logger.info(
                f"Using first shared directory for VM file sharing: {self.shared_path}"
            )

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
            if ":" not in image:
                image = f"{image}:latest"

            if not name:
                # Normalize the name to be used for the VM
                name = image.replace(":", "_")
                # Remove any forward slashes
                name = name.replace("/", "_")

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
        self._original_interface = None  # Keep reference to original interface
        self._tracing_wrapper = None  # Tracing wrapper for interface
        self.use_host_computer_server = use_host_computer_server

        # Initialize tracing
        self._tracing = ComputerTracing(self)

        # Record initialization in telemetry (if enabled)
        if telemetry_enabled and is_telemetry_enabled():
            record_event("computer_initialized", SYSTEM_INFO)
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
                ip_address = self.api_host if self.api_host else "localhost"
                # Create the interface with explicit type annotation
                from .interface.base import BaseComputerInterface

                interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type, ip_address=ip_address, api_port=self.api_port  # type: ignore[arg-type]
                    ),
                )
                self._interface = interface
                self._original_interface = interface

                self.logger.info("Waiting for host computer server to be ready...")
                await self._interface.wait_for_ready()
                self.logger.info("Host computer server ready")
            else:
                # Start or connect to VM
                self.logger.info(f"Starting VM: {self.image}")
                if not self._provider_context:
                    try:
                        provider_type_name = (
                            self.provider_type.name
                            if isinstance(self.provider_type, VMProviderType)
                            else self.provider_type
                        )
                        self.logger.verbose(
                            f"Initializing {provider_type_name} provider context..."
                        )

                        # Explicitly set provider parameters
                        storage = "ephemeral" if self.ephemeral else self.storage
                        verbose = self.verbosity >= LogLevel.DEBUG
                        ephemeral = self.ephemeral
                        port = self.provider_port if self.provider_port is not None else 7777
                        host = self.host if self.host else "localhost"
                        image = self.image
                        shared_path = self.shared_path
                        noVNC_port = self.noVNC_port

                        # Create VM provider instance with explicit parameters
                        try:
                            if self.provider_type == VMProviderType.LUMIER:
                                self.logger.info(f"Using VM image for Lumier provider: {image}")
                                if shared_path:
                                    self.logger.info(
                                        f"Using shared path for Lumier provider: {shared_path}"
                                    )
                                if noVNC_port:
                                    self.logger.info(
                                        f"Using noVNC port for Lumier provider: {noVNC_port}"
                                    )
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
                                    noVNC_port=noVNC_port,
                                )
                            elif self.provider_type == VMProviderType.DOCKER:
                                self.config.vm_provider = VMProviderFactory.create_provider(
                                    self.provider_type,
                                    port=port,
                                    host=host,
                                    storage=storage,
                                    shared_path=shared_path,
                                    image=image or "trycua/cua-ubuntu:latest",
                                    verbose=verbose,
                                    ephemeral=ephemeral,
                                    noVNC_port=noVNC_port,
                                    api_port=self.api_port,
                                )
                            else:
                                raise ValueError(f"Unsupported provider type: {self.provider_type}")
                            self._provider_context = await self.config.vm_provider.__aenter__()
                            self.logger.verbose("VM provider context initialized successfully")
                        except ImportError as ie:
                            self.logger.error(f"Failed to import provider dependencies: {ie}")
                            if str(ie).find("lume") >= 0 and str(ie).find("lumier") < 0:
                                self.logger.error(
                                    "Please install with: pip install cua-computer[lume]"
                                )
                            elif str(ie).find("lumier") >= 0 or str(ie).find("docker") >= 0:
                                self.logger.error(
                                    "Please install with: pip install cua-computer[lumier] and make sure Docker is installed"
                                )
                            elif str(ie).find("cloud") >= 0:
                                self.logger.error(
                                    "Please install with: pip install cua-computer[cloud]"
                                )
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
                    raise RuntimeError(f"VM {self.config.name} could not be found or created.")

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
                            shared_dirs.append({"hostPath": path, "readOnly": False})
                        else:
                            self.logger.warning(f"Shared directory does not exist: {path}")

                    # Prepare run options to pass to the provider
                    run_opts = {}

                    # Add memory and CPU from config
                    if self.config.memory:
                        run_opts["memory"] = self.config.memory
                    if self.config.cpu:
                        run_opts["cpu"] = self.config.cpu

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

                    # Merge custom run_opts
                    run_opts.update(self.custom_run_opts)

                    # Run the VM with the provider
                    try:
                        if self.config.vm_provider is None:
                            raise RuntimeError(
                                f"VM provider not initialized for {self.config.name}"
                            )

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
                            storage=storage_param,
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
                        retry_delay = 3  # 3 seconds between retries for Lumier
                    else:
                        max_retries = 30  # Default for other providers
                        retry_delay = 2  # 2 seconds between retries

                    self.logger.info(
                        f"Waiting up to {max_retries * retry_delay} seconds for VM to be ready..."
                    )
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
            self.logger.error(traceback.format_exc())
            raise RuntimeError(f"Failed to initialize computer: {e}")

        try:
            # Verify we have a valid IP before initializing the interface
            if not ip_address or ip_address == "unknown" or ip_address == "0.0.0.0":
                raise RuntimeError(
                    f"Cannot initialize interface - invalid IP address: {ip_address}"
                )

            # Initialize the interface using the factory with the specified OS
            self.logger.info(f"Initializing interface for {self.os_type} at {ip_address}")
            from .interface.base import BaseComputerInterface

            # Pass authentication credentials if using cloud provider
            if self.provider_type == VMProviderType.CLOUD and self.api_key and self.config.name:
                interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type,
                        ip_address=ip_address,
                        api_key=self.api_key,
                        vm_name=self.config.name,
                        api_port=self.api_port,
                    ),
                )
            else:
                interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type, ip_address=ip_address, api_port=self.api_port
                    ),
                )

            self._interface = interface
            self._original_interface = interface

            # Wait for the WebSocket interface to be ready
            self.logger.info("Connecting to WebSocket interface...")

            try:
                # Use a single timeout for the entire connection process
                # The VM should already be ready at this point, so we're just establishing the connection
                await self._interface.wait_for_ready(timeout=self.timeout)
                self.logger.info("Sandbox interface connected successfully")
            except TimeoutError as e:
                port = getattr(self._interface, "_api_port", 8000)  # Default to 8000 if not set
                self.logger.error(f"Failed to connect to sandbox interface at {ip_address}:{port}")
                raise TimeoutError(
                    f"Could not connect to sandbox interface at {ip_address}:{port}: {str(e)}"
                )

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

            # Record session start in OTEL
            if OTEL_AVAILABLE and is_otel_enabled() and self._telemetry_enabled:
                duration_seconds = time.time() - start_time
                record_operation(
                    operation="computer.session.start",
                    duration_seconds=duration_seconds,
                    status="success",
                    os_type=self.os_type,
                    provider=str(self.provider_type),
                )
                add_breadcrumb(
                    category="computer",
                    message=f"Computer session started ({self.os_type})",
                    level="info",
                    data={
                        "os_type": self.os_type,
                        "provider": str(self.provider_type),
                        "duration_seconds": duration_seconds,
                    },
                )
        except Exception as e:
            # Record failed session start
            if OTEL_AVAILABLE and is_otel_enabled() and self._telemetry_enabled:
                duration_seconds = time.time() - start_time
                record_operation(
                    operation="computer.session.start",
                    duration_seconds=duration_seconds,
                    status="error",
                    os_type=self.os_type,
                )
                capture_exception(
                    e,
                    context={
                        "operation": "computer.session.start",
                        "os_type": self.os_type,
                    },
                )
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
            if (
                not self.use_host_computer_server
                and self._provider_context
                and self.config.vm_provider is not None
            ):
                try:
                    self.logger.info(f"Stopping VM {self.config.name}...")
                    await self.config.vm_provider.stop_vm(
                        name=self.config.name,
                        storage=self.storage,  # Pass storage explicitly for clarity
                    )
                except Exception as e:
                    self.logger.error(f"Error stopping VM: {e}")

                self.logger.verbose("Closing VM provider context...")
                await self.config.vm_provider.__aexit__(None, None, None)
                self._provider_context = None

            await self.disconnect()
            self.logger.info("Computer stopped")

            # Record session stop in OTEL
            if OTEL_AVAILABLE and is_otel_enabled() and self._telemetry_enabled:
                duration_seconds = time.time() - start_time
                record_operation(
                    operation="computer.session.stop",
                    duration_seconds=duration_seconds,
                    status="success",
                    os_type=self.os_type,
                )
                add_breadcrumb(
                    category="computer",
                    message=f"Computer session stopped ({self.os_type})",
                    level="info",
                    data={
                        "os_type": self.os_type,
                        "duration_seconds": duration_seconds,
                    },
                )
        except Exception as e:
            self.logger.debug(
                f"Error during cleanup: {e}"
            )  # Log as debug since this might be expected
        finally:
            # Log stop time for performance monitoring
            duration_ms = (time.time() - start_time) * 1000
            self.logger.debug(f"Computer stop process took {duration_ms:.2f}ms")
        return

    async def start(self) -> None:
        """Start the computer."""
        await self.run()

    async def restart(self) -> None:
        """Restart the computer.

        If using a VM provider that supports restart, this will issue a restart
        without tearing down the provider context, then reconnect the interface.
        Falls back to stop()+run() when a provider restart is not available.
        """
        # Host computer server: just disconnect and run again
        if self.use_host_computer_server:
            try:
                await self.disconnect()
            finally:
                await self.run()
            return

        # If no VM provider context yet, fall back to full run
        if not getattr(self, "_provider_context", None) or self.config.vm_provider is None:
            self.logger.info("No provider context active; performing full restart via run()")
            await self.run()
            return

        # Gracefully close current interface connection if present
        if self._interface:
            try:
                self._interface.close()
            except Exception as e:
                self.logger.debug(f"Error closing interface prior to restart: {e}")

        # Attempt provider-level restart if implemented
        try:
            storage_param = "ephemeral" if self.ephemeral else self.storage
            if hasattr(self.config.vm_provider, "restart_vm"):
                self.logger.info(f"Restarting VM {self.config.name} via provider...")
                await self.config.vm_provider.restart_vm(
                    name=self.config.name, storage=storage_param
                )
            else:
                # Fallback: stop then start without leaving provider context
                self.logger.info(
                    f"Provider has no restart_vm; performing stop+start for {self.config.name}..."
                )
                await self.config.vm_provider.stop_vm(name=self.config.name, storage=storage_param)
                await self.config.vm_provider.run_vm(
                    image=self.image, name=self.config.name, run_opts={}, storage=storage_param
                )
        except Exception as e:
            self.logger.error(f"Failed to restart VM via provider: {e}")
            # As a last resort, do a full stop (with provider context exit) and run
            try:
                await self.stop()
            finally:
                await self.run()
            return

        # Wait for VM to be ready and reconnect interface
        try:
            self.logger.info("Waiting for VM to be ready after restart...")
            if self.provider_type == VMProviderType.LUMIER:
                max_retries = 60
                retry_delay = 3
            else:
                max_retries = 30
                retry_delay = 2
            ip_address = await self.get_ip(max_retries=max_retries, retry_delay=retry_delay)

            self.logger.info(f"Re-initializing interface for {self.os_type} at {ip_address}")
            from .interface.base import BaseComputerInterface

            if self.provider_type == VMProviderType.CLOUD and self.api_key and self.config.name:
                self._interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type,
                        ip_address=ip_address,
                        api_key=self.api_key,
                        vm_name=self.config.name,
                        api_port=self.api_port,
                    ),
                )
            else:
                self._interface = cast(
                    BaseComputerInterface,
                    InterfaceFactory.create_interface_for_os(
                        os=self.os_type,
                        ip_address=ip_address,
                        api_port=self.api_port,
                    ),
                )

            self.logger.info("Connecting to WebSocket interface after restart...")
            await self._interface.wait_for_ready(timeout=self.timeout)
            self.logger.info("Computer reconnected and ready after restart")
        except Exception as e:
            self.logger.error(f"Failed to reconnect after restart: {e}")
            # Try a full reset if reconnection failed
            try:
                await self.stop()
            finally:
                await self.run()

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
            return "127.0.0.1" if not self.api_host else self.api_host

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
            name=self.config.name, storage=storage_param, retry_delay=retry_delay
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
                status = vm.get("status", "unknown") if vm else "unknown"
                ip = vm.get("ip_address") if vm else None
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
        update_opts = {"cpu": cpu or int(self.config.cpu), "memory": memory or self.config.memory}
        if self.config.vm_provider is not None:
            await self.config.vm_provider.update_vm(
                name=self.config.name,
                update_opts=update_opts,
                storage=self.storage,  # Pass storage explicitly for clarity
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
            The computer interface (wrapped with OTEL and/or tracing if enabled)
        """
        if not hasattr(self, "_interface") or self._interface is None:
            error_msg = "Computer interface not initialized. Call run() first."
            self.logger.error(error_msg)
            self.logger.error(
                "Make sure to call await computer.run() before using any interface methods."
            )
            raise RuntimeError(error_msg)

        # Start with the base interface
        result_interface = self._interface

        # Apply OTEL wrapper if enabled and telemetry is on
        if (
            OTEL_AVAILABLE
            and is_otel_enabled()
            and self._telemetry_enabled
            and hasattr(self, "_original_interface")
            and self._original_interface is not None
        ):
            # Create OTEL wrapper if it doesn't exist
            if (
                not hasattr(self, "_otel_wrapper")
                or self._otel_wrapper is None
            ):
                self._otel_wrapper = OtelInterfaceWrapper(
                    self._original_interface, self.os_type
                )
            result_interface = self._otel_wrapper

        # Return tracing wrapper if tracing is active and we have an original interface
        if (
            self._tracing.is_tracing
            and hasattr(self, "_original_interface")
            and self._original_interface is not None
        ):
            # Create wrapper if it doesn't exist or if the original interface changed
            if (
                not hasattr(self, "_tracing_wrapper")
                or self._tracing_wrapper is None
                or self._tracing_wrapper._original_interface != self._original_interface
            ):
                self._tracing_wrapper = TracingInterfaceWrapper(
                    self._original_interface, self._tracing
                )
            return self._tracing_wrapper

        return result_interface

    @property
    def tracing(self) -> ComputerTracing:
        """Get the computer tracing instance for recording sessions.

        Returns:
            ComputerTracing: The tracing instance
        """
        return self._tracing

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

    async def playwright_exec(self, command: str, params: Optional[Dict] = None) -> Dict[str, Any]:
        """
        Execute a Playwright browser command.

        Args:
            command: The browser command to execute (visit_url, click, type, scroll, web_search)
            params: Command parameters

        Returns:
            Dict containing the command result

        Examples:
            # Navigate to a URL
            await computer.playwright_exec("visit_url", {"url": "https://example.com"})

            # Click at coordinates
            await computer.playwright_exec("click", {"x": 100, "y": 200})

            # Type text
            await computer.playwright_exec("type", {"text": "Hello, world!"})

            # Scroll
            await computer.playwright_exec("scroll", {"delta_x": 0, "delta_y": -100})

            # Web search
            await computer.playwright_exec("web_search", {"query": "computer use agent"})
        """
        return await self.interface.playwright_exec(command, params)

    # Add virtual environment management functions to computer interface
    async def venv_install(self, venv_name: str, requirements: list[str]):
        """Install packages in a virtual environment.

        Args:
            venv_name: Name of the virtual environment
            requirements: List of package requirements to install

        Returns:
            Tuple of (stdout, stderr) from the installation command
        """
        requirements = requirements or []
        # Windows vs POSIX handling
        if self.os_type == "windows":
            # Use %USERPROFILE% for home directory and cmd.exe semantics
            venv_path = f"%USERPROFILE%\\.venvs\\{venv_name}"
            ensure_dir_cmd = 'if not exist "%USERPROFILE%\\.venvs" mkdir "%USERPROFILE%\\.venvs"'
            create_cmd = f'if not exist "{venv_path}" python -m venv "{venv_path}"'
            requirements_str = " ".join(requirements)
            # Activate via activate.bat and install
            install_cmd = (
                f'call "{venv_path}\\Scripts\\activate.bat" && pip install {requirements_str}'
                if requirements_str
                else "echo No requirements to install"
            )
            await self.interface.run_command(ensure_dir_cmd)
            await self.interface.run_command(create_cmd)
            return await self.interface.run_command(install_cmd)
        else:
            # POSIX (macOS/Linux)
            venv_path = f"$HOME/.venvs/{venv_name}"
            create_cmd = f'mkdir -p "$HOME/.venvs" && python -m venv "{venv_path}"'
            # Check if venv exists, if not create it
            check_cmd = f'test -d "{venv_path}" || ({create_cmd})'
            _ = await self.interface.run_command(check_cmd)
            # Install packages
            requirements_str = " ".join(requirements)
            install_cmd = (
                f'. "{venv_path}/bin/activate" && pip install {requirements_str}'
                if requirements_str
                else "echo No requirements to install"
            )
        return await self.interface.run_command(install_cmd)

    async def pip_install(self, requirements: list[str]):
        """Install packages using the system Python/pip (no venv).

        Args:
            requirements: List of package requirements to install globally/user site.

        Returns:
            Tuple of (stdout, stderr) from the installation command
        """
        requirements = requirements or []
        if not requirements:
            return await self.interface.run_command("echo No requirements to install")

        # Use python -m pip for cross-platform consistency
        reqs = " ".join(requirements)
        install_cmd = f"python -m pip install {reqs}"
        return await self.interface.run_command(install_cmd)

    async def venv_cmd(self, venv_name: str, command: str):
        """Execute a shell command in a virtual environment.

        Args:
            venv_name: Name of the virtual environment
            command: Shell command to execute in the virtual environment

        Returns:
            Tuple of (stdout, stderr) from the command execution
        """
        if self.os_type == "windows":
            # Windows (cmd.exe)
            venv_path = f"%USERPROFILE%\\.venvs\\{venv_name}"
            # Check existence and signal if missing
            check_cmd = f'if not exist "{venv_path}" (echo VENV_NOT_FOUND) else (echo VENV_FOUND)'
            result = await self.interface.run_command(check_cmd)
            if "VENV_NOT_FOUND" in getattr(result, "stdout", ""):
                # Auto-create the venv with no requirements
                await self.venv_install(venv_name, [])
            # Activate and run the command
            full_command = f'call "{venv_path}\\Scripts\\activate.bat" && {command}'
            return await self.interface.run_command(full_command)
        else:
            # POSIX (macOS/Linux)
            venv_path = f"$HOME/.venvs/{venv_name}"
            # Check if virtual environment exists
            check_cmd = f'test -d "{venv_path}"'
            result = await self.interface.run_command(check_cmd)
            if result.stderr or "test:" in result.stdout:  # venv doesn't exist
                # Auto-create the venv with no requirements
                await self.venv_install(venv_name, [])
            # Activate virtual environment and run command
            full_command = f'. "{venv_path}/bin/activate" && {command}'
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
        import json
        import textwrap

        try:
            func_source = helpers.generate_source_code(python_func)

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
        args_b64 = base64.b64encode(args_json.encode("utf-8")).decode("ascii")
        kwargs_b64 = base64.b64encode(kwargs_json.encode("utf-8")).decode("ascii")

        python_code = f'''
import json
import traceback
import base64

try:
    # Define the function from source
{textwrap.indent(func_source, "    ")}
    
    # Deserialize args and kwargs from base64 JSON
    _args_b64 = """{args_b64}"""
    _kwargs_b64 = """{kwargs_b64}"""
    args = json.loads(base64.b64decode(_args_b64).decode('utf-8'))
    kwargs = json.loads(base64.b64decode(_kwargs_b64).decode('utf-8'))
    
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
        encoded_code = base64.b64encode(python_code.encode("utf-8")).decode("ascii")

        # Execute the Python code in the virtual environment
        python_command = (
            f"python -c \"import base64; exec(base64.b64decode('{encoded_code}').decode('utf-8'))\""
        )
        result = await self.venv_cmd(venv_name, python_command)

        # Parse the output to extract the payload
        start_marker = "<<<VENV_EXEC_START>>>"
        end_marker = "<<<VENV_EXEC_END>>>"

        # Print original stdout
        print(result.stdout[: result.stdout.find(start_marker)])

        if start_marker in result.stdout and end_marker in result.stdout:
            start_idx = result.stdout.find(start_marker) + len(start_marker)
            end_idx = result.stdout.find(end_marker)

            if start_idx < end_idx:
                output_json = result.stdout[start_idx:end_idx]

                try:
                    # Decode and deserialize the output payload from JSON
                    output_payload = json.loads(output_json)
                except Exception as e:
                    raise Exception(f"Failed to decode output payload: {e}")

                if output_payload["success"]:
                    return output_payload["result"]
                else:
                    import builtins

                    # Recreate and raise the original exception
                    error_info = output_payload.get("error", {}) or {}
                    err_type = error_info.get("type") or "Exception"
                    err_msg = error_info.get("message") or ""
                    err_tb = error_info.get("traceback") or ""

                    exc_cls = getattr(builtins, err_type, None)
                    if isinstance(exc_cls, type) and issubclass(exc_cls, BaseException):
                        # Built-in exception: rethrow with remote traceback appended
                        raise exc_cls(f"{err_msg}\n\nRemote traceback:\n{err_tb}")
                    else:
                        # Non built-in: raise a safe local error carrying full remote context
                        raise RuntimeError(f"{err_type}: {err_msg}\n\nRemote traceback:\n{err_tb}")
            else:
                raise Exception("Invalid output format: markers found but no content between them")
        else:
            # Fallback: return stdout/stderr if no payload markers found
            raise Exception(
                f"No output payload found. stdout: {result.stdout}, stderr: {result.stderr}"
            )

    async def venv_exec_background(
        self, venv_name: str, python_func, *args, requirements: Optional[List[str]] = None, **kwargs
    ) -> int:
        """Run the Python function in the venv in the background and return the PID.

        Uses a short launcher Python that spawns a detached child and exits immediately.
        """
        import base64
        import json
        import textwrap
        import time as _time

        try:
            func_source = helpers.generate_source_code(python_func)
            func_name = python_func.__name__
            args_json = json.dumps(args, default=str)
            kwargs_json = json.dumps(kwargs, default=str)
        except OSError as e:
            raise Exception(f"Cannot retrieve source code for function {python_func.__name__}: {e}")
        except Exception as e:
            raise Exception(f"Failed to reconstruct function source: {e}")

        reqs_list = requirements or []
        reqs_json = json.dumps(reqs_list)

        # Create Python code that will define and execute the function
        args_b64 = base64.b64encode(args_json.encode("utf-8")).decode("ascii")
        kwargs_b64 = base64.b64encode(kwargs_json.encode("utf-8")).decode("ascii")

        payload_code = (
            f'''
import json
import traceback
import base64

try:
    # Define the function from source
{textwrap.indent(func_source, "    ")}
    
    # Deserialize args and kwargs from base64 JSON
    _args_b64 = """{args_b64}"""
    _kwargs_b64 = """{kwargs_b64}"""
    args = json.loads(base64.b64decode(_args_b64).decode('utf-8'))
    kwargs = json.loads(base64.b64decode(_kwargs_b64).decode('utf-8'))
    
    # Ensure requirements inside the active venv
    for pkg in json.loads('''
            + repr(reqs_json)
            + """):
        if pkg:
            import subprocess, sys
            subprocess.run([sys.executable, '-m', 'pip', 'install', pkg], check=False)
    _ = {func_name}(*args, **kwargs)
except Exception:
    import sys
    sys.stderr.write(traceback.format_exc())
"""
        )
        payload_b64 = base64.b64encode(payload_code.encode("utf-8")).decode("ascii")

        if self.os_type == "windows":
            # Launcher spawns detached child and prints its PID
            launcher_code = f"""
import base64, subprocess, os, sys
DETACHED_PROCESS = 0x00000008
CREATE_NEW_PROCESS_GROUP = 0x00000200
creationflags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
code = base64.b64decode("{payload_b64}").decode("utf-8")
p = subprocess.Popen(["python", "-c", code], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=creationflags)
print(p.pid)
"""
            launcher_b64 = base64.b64encode(launcher_code.encode("utf-8")).decode("ascii")
            venv_path = f"%USERPROFILE%\\.venvs\\{venv_name}"
            cmd = (
                'cmd /c "'
                f'call "{venv_path}\\Scripts\\activate.bat" && '
                f"python -c \"import base64; exec(base64.b64decode('{launcher_b64}').decode('utf-8'))\""
                '"'
            )
            result = await self.interface.run_command(cmd)
            pid_str = (result.stdout or "").strip().splitlines()[-1].strip()
            return int(pid_str)
        else:
            log = f"/tmp/cua_bg_{int(_time.time())}.log"
            launcher_code = f"""
import base64, subprocess, os, sys
code = base64.b64decode("{payload_b64}").decode("utf-8")
with open("{log}", "ab", buffering=0) as f:
    p = subprocess.Popen(["python", "-c", code], stdout=f, stderr=subprocess.STDOUT, preexec_fn=getattr(os, "setsid", None))
print(p.pid)
"""
            launcher_b64 = base64.b64encode(launcher_code.encode("utf-8")).decode("ascii")
            venv_path = f"$HOME/.venvs/{venv_name}"
            shell = (
                f'. "{venv_path}/bin/activate" && '
                f"python -c \"import base64; exec(base64.b64decode('{launcher_b64}').decode('utf-8'))\""
            )
            result = await self.interface.run_command(shell)
            pid_str = (result.stdout or "").strip().splitlines()[-1].strip()
            return int(pid_str)

    async def python_exec(self, python_func, *args, **kwargs):
        """Execute a Python function using the system Python (no venv).

        Uses source extraction and base64 transport, mirroring venv_exec but
        without virtual environment activation.

        Returns the function result or raises a reconstructed exception with
        remote traceback context appended.
        """
        import base64
        import json
        import textwrap

        try:
            func_source = helpers.generate_source_code(python_func)
            func_name = python_func.__name__
            args_json = json.dumps(args, default=str)
            kwargs_json = json.dumps(kwargs, default=str)
        except OSError as e:
            raise Exception(f"Cannot retrieve source code for function {python_func.__name__}: {e}")
        except Exception as e:
            raise Exception(f"Failed to reconstruct function source: {e}")

        # Create Python code that will define and execute the function
        args_b64 = base64.b64encode(args_json.encode("utf-8")).decode("ascii")
        kwargs_b64 = base64.b64encode(kwargs_json.encode("utf-8")).decode("ascii")

        python_code = f'''
import json
import traceback
import base64

try:
    # Define the function from source
{textwrap.indent(func_source, "    ")}
    
    # Deserialize args and kwargs from base64 JSON
    _args_b64 = """{args_b64}"""
    _kwargs_b64 = """{kwargs_b64}"""
    args = json.loads(base64.b64decode(_args_b64).decode('utf-8'))
    kwargs = json.loads(base64.b64decode(_kwargs_b64).decode('utf-8'))
    
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

        encoded_code = base64.b64encode(python_code.encode("utf-8")).decode("ascii")
        python_command = (
            f"python -c \"import base64; exec(base64.b64decode('{encoded_code}').decode('utf-8'))\""
        )
        result = await self.interface.run_command(python_command)

        start_marker = "<<<VENV_EXEC_START>>>"
        end_marker = "<<<VENV_EXEC_END>>>"

        print(result.stdout[: result.stdout.find(start_marker)])

        if start_marker in result.stdout and end_marker in result.stdout:
            start_idx = result.stdout.find(start_marker) + len(start_marker)
            end_idx = result.stdout.find(end_marker)
            if start_idx < end_idx:
                output_json = result.stdout[start_idx:end_idx]
                try:
                    output_payload = json.loads(output_json)
                except Exception as e:
                    raise Exception(f"Failed to decode output payload: {e}")

                if output_payload["success"]:
                    return output_payload["result"]
                else:
                    import builtins

                    error_info = output_payload.get("error", {}) or {}
                    err_type = error_info.get("type") or "Exception"
                    err_msg = error_info.get("message") or ""
                    err_tb = error_info.get("traceback") or ""
                    exc_cls = getattr(builtins, err_type, None)
                    if isinstance(exc_cls, type) and issubclass(exc_cls, BaseException):
                        raise exc_cls(f"{err_msg}\n\nRemote traceback:\n{err_tb}")
                    else:
                        raise RuntimeError(f"{err_type}: {err_msg}\n\nRemote traceback:\n{err_tb}")
            else:
                raise Exception("Invalid output format: markers found but no content between them")
        else:
            raise Exception(
                f"No output payload found. stdout: {result.stdout}, stderr: {result.stderr}"
            )

    async def python_exec_background(
        self, python_func, *args, requirements: Optional[List[str]] = None, **kwargs
    ) -> int:
        """Run a Python function with the system interpreter in the background and return PID.

        Uses a short launcher Python that spawns a detached child and exits immediately.
        """
        import base64
        import json
        import textwrap
        import time as _time

        try:
            func_source = helpers.generate_source_code(python_func)
            func_name = python_func.__name__
            args_json = json.dumps(args, default=str)
            kwargs_json = json.dumps(kwargs, default=str)
        except OSError as e:
            raise Exception(f"Cannot retrieve source code for function {python_func.__name__}: {e}")
        except Exception as e:
            raise Exception(f"Failed to reconstruct function source: {e}")

        # Create Python code that will define and execute the function
        args_b64 = base64.b64encode(args_json.encode("utf-8")).decode("ascii")
        kwargs_b64 = base64.b64encode(kwargs_json.encode("utf-8")).decode("ascii")

        payload_code = f'''
import json
import traceback
import base64

try:
    # Define the function from source
{textwrap.indent(func_source, "    ")}
    
    # Deserialize args and kwargs from base64 JSON
    _args_b64 = """{args_b64}"""
    _kwargs_b64 = """{kwargs_b64}"""
    args = json.loads(base64.b64decode(_args_b64).decode('utf-8'))
    kwargs = json.loads(base64.b64decode(_kwargs_b64).decode('utf-8'))
    
    _ = {func_name}(*args, **kwargs)
except Exception:
    import sys
    sys.stderr.write(traceback.format_exc())
'''
        payload_b64 = base64.b64encode(payload_code.encode("utf-8")).decode("ascii")

        if self.os_type == "windows":
            launcher_code = f"""
import base64, subprocess, os, sys
DETACHED_PROCESS = 0x00000008
CREATE_NEW_PROCESS_GROUP = 0x00000200
creationflags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
code = base64.b64decode("{payload_b64}").decode("utf-8")
p = subprocess.Popen(["python", "-c", code], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=creationflags)
print(p.pid)
"""
            launcher_b64 = base64.b64encode(launcher_code.encode("utf-8")).decode("ascii")
            cmd = f"python -c \"import base64; exec(base64.b64decode('{launcher_b64}').decode('utf-8'))\""
            result = await self.interface.run_command(cmd)
            pid_str = (result.stdout or "").strip().splitlines()[-1].strip()
            return int(pid_str)
        else:
            log = f"/tmp/cua_bg_{int(_time.time())}.log"
            launcher_code = f"""
import base64, subprocess, os, sys
code = base64.b64decode("{payload_b64}").decode("utf-8")
with open("{log}", "ab", buffering=0) as f:
    p = subprocess.Popen(["python", "-c", code], stdout=f, stderr=subprocess.STDOUT, preexec_fn=getattr(os, "setsid", None))
print(p.pid)
"""
            launcher_b64 = base64.b64encode(launcher_code.encode("utf-8")).decode("ascii")
            cmd = f"python -c \"import base64; exec(base64.b64decode('{launcher_b64}').decode('utf-8'))\""
            result = await self.interface.run_command(cmd)
            pid_str = (result.stdout or "").strip().splitlines()[-1].strip()
            return int(pid_str)

    def python_command(
        self,
        requirements: Optional[List[str]] = None,
        *,
        venv_name: str = "default",
        use_system_python: bool = False,
        background: bool = False,
    ) -> Callable[[Callable[P, R]], Callable[P, Awaitable[R]]]:
        """Decorator to execute a Python function remotely in this Computer's venv.

        This mirrors `computer.helpers.sandboxed()` but binds to this instance and
        optionally ensures required packages are installed before execution.

        Args:
            requirements: Packages to install in the virtual environment.
            venv_name: Name of the virtual environment to use.
            use_system_python: If True, use the system Python/pip instead of a venv.
            background: If True, run the function detached and return the child PID immediately.

        Returns:
            A decorator that turns a local function into an async callable which
            runs remotely and returns the function's result.
        """

        reqs = list(requirements or [])

        def decorator(func: Callable[P, R]) -> Callable[P, Awaitable[R]]:
            @wraps(func)
            async def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
                if use_system_python:
                    # For background, avoid blocking installs; install inside child process
                    if background:
                        return await self.python_exec_background(func, *args, requirements=reqs, **kwargs)  # type: ignore[return-value]
                    # Foreground: install first, then execute
                    if reqs:
                        await self.pip_install(reqs)
                    return await self.python_exec(func, *args, **kwargs)
                else:
                    # For background, avoid blocking installs; install inside child process under venv
                    if background:
                        return await self.venv_exec_background(venv_name, func, *args, requirements=reqs, **kwargs)  # type: ignore[return-value]
                    # Foreground: ensure venv and install, then execute
                    await self.venv_install(venv_name, reqs)
                    return await self.venv_exec(venv_name, func, *args, **kwargs)

            return wrapper

        return decorator
