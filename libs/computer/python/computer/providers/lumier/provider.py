"""
Lumier VM provider implementation.

This provider uses Docker containers running the Lumier image to create
macOS and Linux VMs. It handles VM lifecycle operations through Docker
commands and container management.
"""

import logging
import os
import json
import asyncio
from typing import Dict, List, Optional, Any
import subprocess
import time
import re

from ..base import BaseVMProvider, VMProviderType
from ..lume_api import (
    lume_api_get,
    lume_api_run,
    lume_api_stop,
    lume_api_update
)

# Setup logging
logger = logging.getLogger(__name__)

# Check if Docker is available
try:
    subprocess.run(["docker", "--version"], capture_output=True, check=True)
    HAS_LUMIER = True
except (subprocess.SubprocessError, FileNotFoundError):
    HAS_LUMIER = False


class LumierProvider(BaseVMProvider):
    """
    Lumier VM Provider implementation using Docker containers.
    
    This provider uses Docker to run Lumier containers that can create
    macOS and Linux VMs through containerization.
    """
    
    def __init__(
        self, 
        port: Optional[int] = 7777,
        host: str = "localhost",
        storage: Optional[str] = None,  # Can be a path or 'ephemeral'
        shared_path: Optional[str] = None,
        image: str = "macos-sequoia-cua:latest",  # VM image to use
        verbose: bool = False,
        ephemeral: bool = False,
        noVNC_port: Optional[int] = 8006,
    ):
        """Initialize the Lumier VM Provider.
        
        Args:
            port: Port for the API server (default: 7777)
            host: Hostname for the API server (default: localhost)
            storage: Path for persistent VM storage
            shared_path: Path for shared folder between host and VM
            image: VM image to use (e.g. "macos-sequoia-cua:latest")
            verbose: Enable verbose logging
            ephemeral: Use ephemeral (temporary) storage
            noVNC_port: Specific port for noVNC interface (default: 8006)
        """
        self.host = host
        # Always ensure api_port has a valid value (7777 is the default)
        self.api_port = 7777 if port is None else port
        self.vnc_port = noVNC_port  # User-specified noVNC port, will be set in run_vm if provided
        self.ephemeral = ephemeral
        
        # Handle ephemeral storage (temporary directory)
        if ephemeral:
            self.storage = "ephemeral"
        else:
            self.storage = storage
            
        self.shared_path = shared_path
        self.image = image  # Store the VM image name to use
        # The container_name will be set in run_vm using the VM name
        self.verbose = verbose
        self._container_id = None
        self._api_url = None  # Will be set after container starts
        
    @property
    def provider_type(self) -> VMProviderType:
        """Return the provider type."""
        return VMProviderType.LUMIER
    
    def _parse_memory(self, memory_str: str) -> int:
        """Parse memory string to MB integer.
        
        Examples:
            "8GB" -> 8192
            "1024MB" -> 1024
            "512" -> 512
        """
        if isinstance(memory_str, int):
            return memory_str
            
        if isinstance(memory_str, str):
            # Extract number and unit
            match = re.match(r"(\d+)([A-Za-z]*)", memory_str)
            if match:
                value, unit = match.groups()
                value = int(value)
                unit = unit.upper()
                
                if unit == "GB" or unit == "G":
                    return value * 1024
                elif unit == "MB" or unit == "M" or unit == "":
                    return value
                    
        # Default fallback
        logger.warning(f"Could not parse memory string '{memory_str}', using 8GB default")
        return 8192  # Default to 8GB
    
    # Helper methods for interacting with the Lumier API through curl
    # These methods handle the various VM operations via API calls
    
    def _get_curl_error_message(self, return_code: int) -> str:
        """Get a descriptive error message for curl return codes.
        
        Args:
            return_code: The curl return code
            
        Returns:
            A descriptive error message
        """
        # Map common curl error codes to helpful messages
        if return_code == 7:
            return "Failed to connect - API server is starting up"
        elif return_code == 22:
            return "HTTP error returned from API server"
        elif return_code == 28:
            return "Operation timeout - API server is slow to respond"
        elif return_code == 52:
            return "Empty reply from server - API is starting but not ready"
        elif return_code == 56:
            return "Network problem during data transfer"
        else:
            return f"Unknown curl error code: {return_code}"

    
    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Get VM information by name.
        
        Args:
            name: Name of the VM to get information for
            storage: Optional storage path override. If provided, this will be used
                    instead of the provider's default storage path.
            
        Returns:
            Dictionary with VM information including status, IP address, etc.
        """
        if not HAS_LUMIER:
            logger.error("Docker is not available. Cannot get VM status.")
            return {
                "name": name,
                "status": "unavailable",
                "error": "Docker is not available"
            }
            
        # Store the current name for API requests
        self.container_name = name
        
        try:
            # Check if the container exists and is running
            check_cmd = ["docker", "ps", "-a", "--filter", f"name={name}", "--format", "{{.Status}}"]
            check_result = subprocess.run(check_cmd, capture_output=True, text=True)
            container_status = check_result.stdout.strip()
            
            if not container_status:
                logger.info(f"Container {name} does not exist. Will create when run_vm is called.")
                return {
                    "name": name,
                    "status": "not_found",
                    "message": "Container doesn't exist yet"
                }
                
            # Container exists, check if it's running
            is_running = container_status.startswith("Up")
            
            if not is_running:
                logger.info(f"Container {name} exists but is not running. Status: {container_status}")
                return {
                    "name": name,
                    "status": "stopped",
                    "container_status": container_status,
                }
                
            # Container is running, get the IP address and API status from Lumier API
            logger.info(f"Container {name} is running. Getting VM status from API.")
            
            # Use the shared lume_api_get function directly
            vm_info = lume_api_get(
                vm_name=name,
                host=self.host,
                port=self.api_port,
                storage=storage if storage is not None else self.storage,
                debug=self.verbose,
                verbose=self.verbose
            )
            
            # Check for API errors
            if "error" in vm_info:
                # Use debug level instead of warning to reduce log noise during polling
                logger.debug(f"API request error: {vm_info['error']}")
                return {
                    "name": name,
                    "status": "running",  # Container is running even if API is not responsive
                    "api_status": "error",
                    "error": vm_info["error"],
                    "container_status": container_status
                }
                
            # Process the VM status information
            vm_status = vm_info.get("status", "unknown")
            vnc_url = vm_info.get("vncUrl", "")
            ip_address = vm_info.get("ipAddress", "")
            
            # IMPORTANT: Always ensure we have a valid IP address for connectivity
            # If the API doesn't return an IP address, default to localhost (127.0.0.1)
            # This makes the behavior consistent with LumeProvider
            if not ip_address and vm_status == "running":
                ip_address = "127.0.0.1"
                logger.info(f"No IP address returned from API, defaulting to {ip_address}")
                vm_info["ipAddress"] = ip_address
            
            logger.info(f"VM {name} status: {vm_status}")
            
            if ip_address and vnc_url:
                logger.info(f"VM {name} has IP: {ip_address} and VNC URL: {vnc_url}")
            elif not ip_address and not vnc_url and vm_status != "running":
                # Not running is expected in this case
                logger.info(f"VM {name} is not running yet. Status: {vm_status}")
            else:
                # Missing IP or VNC but status is running - this is unusual but handled with default IP
                logger.warning(f"VM {name} is running but missing expected fields. API response: {vm_info}")
            
            # Return the full status information
            return {
                "name": name,
                "status": vm_status,
                "ip_address": ip_address,
                "vnc_url": vnc_url,
                "api_status": "ok",
                "container_status": container_status,
                **vm_info  # Include all fields from the API response
            }
        except subprocess.SubprocessError as e:
            logger.error(f"Failed to check container status: {e}")
            return {
                "name": name,
                "status": "error",
                "error": f"Failed to check container status: {str(e)}"
            }
    
    async def list_vms(self) -> List[Dict[str, Any]]:
        """List all VMs managed by this provider.
        
        For Lumier provider, there is only one VM per container.
        """
        try:
            status = await self.get_vm("default")
            return [status] if status.get("status") != "unknown" else []
        except Exception as e:
            logger.error(f"Failed to list VMs: {e}")
            return []
    
    async def run_vm(self, image: str, name: str, run_opts: Dict[str, Any], storage: Optional[str] = None) -> Dict[str, Any]:
        """Run a VM with the given options.
        
        Args:
            image: Name/tag of the image to use
            name: Name of the VM to run (used for the container name and Docker image tag)
            run_opts: Options for running the VM, including:
                - cpu: Number of CPU cores
                - memory: Amount of memory (e.g. "8GB")
                - noVNC_port: Specific port for noVNC interface
        
        Returns:
            Dictionary with VM status information
        """
        # Set the container name using the VM name for consistency
        self.container_name = name
        try:
            # First, check if container already exists and remove it
            try:
                check_cmd = ["docker", "ps", "-a", "--filter", f"name={self.container_name}", "--format", "{{.ID}}"]
                check_result = subprocess.run(check_cmd, capture_output=True, text=True)
                existing_container = check_result.stdout.strip()
                
                if existing_container:
                    logger.info(f"Removing existing container: {self.container_name}")
                    remove_cmd = ["docker", "rm", "-f", self.container_name]
                    subprocess.run(remove_cmd, check=True)
            except subprocess.CalledProcessError as e:
                logger.warning(f"Error removing existing container: {e}")
                # Continue anyway, next steps will fail if there's a real problem
            
            # Prepare the Docker run command
            cmd = ["docker", "run", "-d", "--name", self.container_name]
            
            cmd.extend(["-p", f"{self.vnc_port}:8006"])
            logger.debug(f"Using specified noVNC_port: {self.vnc_port}")
                
            # Set API URL using the API port
            self._api_url = f"http://{self.host}:{self.api_port}"
            
            # Parse memory setting
            memory_mb = self._parse_memory(run_opts.get("memory", "8GB"))
            
            # Add storage volume mount if storage is specified (for persistent VM storage)
            if self.storage and self.storage != "ephemeral":
                # Create storage directory if it doesn't exist
                storage_dir = os.path.abspath(os.path.expanduser(self.storage or ""))
                os.makedirs(storage_dir, exist_ok=True)
                
                # Add volume mount for storage
                cmd.extend([
                    "-v", f"{storage_dir}:/storage", 
                    "-e", f"HOST_STORAGE_PATH={storage_dir}"
                ])
                logger.debug(f"Using persistent storage at: {storage_dir}")
            
            # Add shared folder volume mount if shared_path is specified
            if self.shared_path:
                # Create shared directory if it doesn't exist
                shared_dir = os.path.abspath(os.path.expanduser(self.shared_path or ""))
                os.makedirs(shared_dir, exist_ok=True)
                
                # Add volume mount for shared folder
                cmd.extend([
                    "-v", f"{shared_dir}:/shared",
                    "-e", f"HOST_SHARED_PATH={shared_dir}"
                ])
                logger.debug(f"Using shared folder at: {shared_dir}")
            
            # Add environment variables
            # Always use the container_name as the VM_NAME for consistency
            # Use the VM image passed from the Computer class
            logger.debug(f"Using VM image: {self.image}")
            
            # If ghcr.io is in the image, use the full image name
            if "ghcr.io" in self.image:
                vm_image = self.image
            else:
                vm_image = f"ghcr.io/trycua/{self.image}"

            cmd.extend([
                "-e", f"VM_NAME={self.container_name}",
                "-e", f"VERSION={vm_image}",
                "-e", f"CPU_CORES={run_opts.get('cpu', '4')}",
                "-e", f"RAM_SIZE={memory_mb}",
            ])
            
            # Specify the Lumier image with the full image name
            lumier_image = "trycua/lumier:latest"
            
            # First check if the image exists locally
            try:
                logger.debug(f"Checking if Docker image {lumier_image} exists locally...")
                check_image_cmd = ["docker", "image", "inspect", lumier_image]
                subprocess.run(check_image_cmd, capture_output=True, check=True)
                logger.debug(f"Docker image {lumier_image} found locally.")
            except subprocess.CalledProcessError:
                # Image doesn't exist locally
                logger.warning(f"\nWARNING: Docker image {lumier_image} not found locally.")
                logger.warning("The system will attempt to pull it from Docker Hub, which may fail if you have network connectivity issues.")
                logger.warning("If the Docker pull fails, you may need to manually pull the image first with:")
                logger.warning(f"  docker pull {lumier_image}\n")
            
            # Add the image to the command
            cmd.append(lumier_image)
            
            # Print the Docker command for debugging
            logger.debug(f"DOCKER COMMAND: {' '.join(cmd)}")
            
            # Run the container with improved error handling
            try:
                result = subprocess.run(cmd, capture_output=True, text=True, check=True)
            except subprocess.CalledProcessError as e:
                if "no route to host" in str(e.stderr).lower() or "failed to resolve reference" in str(e.stderr).lower():
                    error_msg = (f"Network error while trying to pull Docker image '{lumier_image}'\n"
                                f"Error: {e.stderr}\n\n"
                                f"SOLUTION: Please try one of the following:\n"
                                f"1. Check your internet connection\n"
                                f"2. Pull the image manually with: docker pull {lumier_image}\n"
                                f"3. Check if Docker is running properly\n")
                    logger.error(error_msg)
                    raise RuntimeError(error_msg)
                raise
            
            # Container started, now check VM status with polling
            logger.debug("Container started, checking VM status...")
            logger.debug("NOTE: This may take some time while the VM image is being pulled and initialized")
            
            # Start a background thread to show container logs in real-time
            import threading
            
            def show_container_logs():
                # Give the container a moment to start generating logs
                time.sleep(1)
                logger.debug(f"\n---- CONTAINER LOGS FOR '{name}' (LIVE) ----")
                logger.debug("Showing logs as they are generated. Press Ctrl+C to stop viewing logs...\n")
                
                try:
                    # Use docker logs with follow option
                    log_cmd = ["docker", "logs", "--tail", "30", "--follow", name]
                    process = subprocess.Popen(log_cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, 
                                              text=True, bufsize=1, universal_newlines=True)
                    
                    # Read and print logs line by line
                    for line in process.stdout:
                        logger.debug(line, end='')
                        
                        # Break if process has exited
                        if process.poll() is not None:
                            break
                except Exception as e:
                    logger.error(f"\nError showing container logs: {e}")
                    if self.verbose:
                        logger.error(f"Error in log streaming thread: {e}")
                finally:
                    logger.debug("\n---- LOG STREAMING ENDED ----")
                    # Make sure process is terminated
                    if 'process' in locals() and process.poll() is None:
                        process.terminate()
            
            # Start log streaming in a background thread if verbose mode is enabled
            log_thread = threading.Thread(target=show_container_logs)
            log_thread.daemon = True  # Thread will exit when main program exits
            log_thread.start()
            
            # Skip waiting for container readiness and just poll get_vm directly
            # Poll the get_vm method indefinitely until the VM is ready with an IP address
            attempt = 0
            consecutive_errors = 0
            vm_running = False
            
            while True:  # Wait indefinitely
                try:
                    # Use longer delays to give the system time to initialize
                    if attempt > 0:
                        # Start with 5s delay, then increase gradually up to 30s for later attempts
                        # But use shorter delays while we're getting API errors
                        if consecutive_errors > 0 and consecutive_errors < 5:
                            wait_time = 3  # Use shorter delays when we're getting API errors
                        else:  
                            wait_time = min(30, 5 + (attempt * 2))
                        
                        logger.debug(f"Waiting {wait_time}s before retry #{attempt+1}...")
                        await asyncio.sleep(wait_time)
                    
                    # Try to get VM status
                    logger.debug(f"Checking VM status (attempt {attempt+1})...")
                    vm_status = await self.get_vm(name)
                    
                    # Check for API errors
                    if 'error' in vm_status:
                        consecutive_errors += 1
                        error_msg = vm_status.get('error', 'Unknown error')
                        
                        # Only print a user-friendly status message, not the raw error
                        # since _lume_api_get already logged the technical details
                        if consecutive_errors == 1 or attempt % 5 == 0:
                            if 'Empty reply from server' in error_msg:
                                logger.info("API server is starting up - container is running, but API isn't fully initialized yet.")
                                logger.info("This is expected during the initial VM setup - will continue polling...")
                            else:
                                # Don't repeat the exact same error message each time
                                logger.warning(f"API request error (attempt {attempt+1}): {error_msg}")
                                # Just log that we're still working on it
                                if attempt > 3:
                                    logger.debug("Still waiting for the API server to become available...")
                            
                        # If we're getting errors but container is running, that's normal during startup
                        if vm_status.get('status') == 'running':
                            if not vm_running:
                                logger.info("Container is running, waiting for the VM within it to become fully ready...")
                                logger.info("This might take a minute while the VM initializes...")
                                vm_running = True
                        
                        # Increase counter and continue
                        attempt += 1
                        continue
                    
                    # Reset consecutive error counter when we get a successful response
                    consecutive_errors = 0
                    
                    # If the VM is running, check if it has an IP address (which means it's fully ready)
                    if vm_status.get('status') == 'running':
                        vm_running = True
                        
                        # Check if we have an IP address, which means the VM is fully ready
                        if 'ip_address' in vm_status and vm_status['ip_address']:
                            logger.info(f"VM is now fully running with IP: {vm_status.get('ip_address')}")
                            if 'vnc_url' in vm_status and vm_status['vnc_url']:
                                logger.info(f"VNC URL: {vm_status.get('vnc_url')}")
                            return vm_status
                        else:
                            logger.debug("VM is running but still initializing network interfaces...")
                            logger.debug("Waiting for IP address to be assigned...")
                    else:
                        # VM exists but might still be starting up
                        status = vm_status.get('status', 'unknown')
                        logger.debug(f"VM found but status is: {status}. Continuing to poll...")
                    
                    # Increase counter for next iteration's delay calculation
                    attempt += 1
                    
                    # If we reach a very large number of attempts, give a reassuring message but continue
                    if attempt % 10 == 0:
                        logger.debug(f"Still waiting after {attempt} attempts. This might take several minutes for first-time setup.")
                        if not vm_running and attempt >= 20:
                            logger.warning("\nNOTE: First-time VM initialization can be slow as images are downloaded.")
                            logger.warning("If this continues for more than 10 minutes, you may want to check:")
                            logger.warning("  1. Docker logs with: docker logs " + name)
                            logger.warning("  2. If your network can access container registries")
                            logger.warning("Press Ctrl+C to abort if needed.\n")
                            
                    # After 150 attempts (likely over 30-40 minutes), return current status
                    if attempt >= 150:
                        logger.debug(f"Reached 150 polling attempts. VM status is: {vm_status.get('status', 'unknown')}")
                        logger.debug("Returning current VM status, but please check Docker logs if there are issues.")
                        return vm_status
                    
                except Exception as e:
                    # Always continue retrying, but with increasing delays
                    logger.warning(f"Error checking VM status (attempt {attempt+1}): {e}. Will retry.")
                    consecutive_errors += 1
                    
                    # If we've had too many consecutive errors, might be a deeper problem
                    if consecutive_errors >= 10:
                        logger.warning(f"\nWARNING: Encountered {consecutive_errors} consecutive errors while checking VM status.")
                        logger.warning("You may need to check the Docker container logs or restart the process.")
                        logger.warning(f"Error details: {str(e)}\n")
                        
                    # Increase attempt counter for next iteration
                    attempt += 1
                    
                    # After many consecutive errors, add a delay to avoid hammering the system
                    if attempt > 5:
                        error_delay = min(30, 10 + attempt)
                        logger.warning(f"Multiple connection errors, waiting {error_delay}s before next attempt...")
                        await asyncio.sleep(error_delay)
        
        except subprocess.CalledProcessError as e:
            error_msg = f"Failed to start Lumier container: {e.stderr if hasattr(e, 'stderr') else str(e)}"
            logger.error(error_msg)
            raise RuntimeError(error_msg)
        
    async def _wait_for_container_ready(self, container_name: str, timeout: int = 90) -> bool:
        """Wait for the Lumier container to be fully ready with a valid API response.
        
        Args:
            container_name: Name of the Docker container to check
            timeout: Maximum time to wait in seconds (default: 90 seconds)
            
        Returns:
            True if the container is running, even if API is not fully ready.
            This allows operations to continue with appropriate fallbacks.
        """
        start_time = time.time()
        api_ready = False
        container_running = False
        
        logger.debug(f"Waiting for container {container_name} to be ready (timeout: {timeout}s)...")
        
        while time.time() - start_time < timeout:
            # Check if container is running
            try:
                check_cmd = ["docker", "ps", "--filter", f"name={container_name}", "--format", "{{.Status}}"]
                result = subprocess.run(check_cmd, capture_output=True, text=True, check=True)
                container_status = result.stdout.strip()
                
                if container_status and container_status.startswith("Up"):
                    container_running = True
                    logger.info(f"Container {container_name} is running with status: {container_status}")
                else:
                    logger.warning(f"Container {container_name} not yet running, status: {container_status}")
                    # container is not running yet, wait and try again
                    await asyncio.sleep(2)  # Longer sleep to give Docker time
                    continue
            except subprocess.CalledProcessError as e:
                logger.warning(f"Error checking container status: {e}")
                await asyncio.sleep(2)
                continue
                
            # Container is running, check if API is responsive
            try:
                # First check the health endpoint
                api_url = f"http://{self.host}:{self.api_port}/health"
                logger.info(f"Checking API health at: {api_url}")
                
                # Use longer timeout for API health check since it may still be initializing
                curl_cmd = ["curl", "-s", "--connect-timeout", "5", "--max-time", "10", api_url]
                result = subprocess.run(curl_cmd, capture_output=True, text=True)
                
                if result.returncode == 0 and "ok" in result.stdout.lower():
                    api_ready = True
                    logger.info(f"API is ready at {api_url}")
                    break
                else:
                    # API health check failed, now let's check if the VM status endpoint is responsive
                    # This covers cases where the health endpoint isn't implemented but the VM API is working
                    vm_api_url = f"http://{self.host}:{self.api_port}/lume/vms/{container_name}"
                    if self.storage:
                        import urllib.parse
                        encoded_storage = urllib.parse.quote_plus(self.storage)
                        vm_api_url += f"?storage={encoded_storage}"
                        
                    curl_vm_cmd = ["curl", "-s", "--connect-timeout", "5", "--max-time", "10", vm_api_url]
                    vm_result = subprocess.run(curl_vm_cmd, capture_output=True, text=True)
                    
                    if vm_result.returncode == 0 and vm_result.stdout.strip():
                        # VM API responded with something - consider the API ready
                        api_ready = True
                        logger.info(f"VM API is ready at {vm_api_url}")
                        break
                    else:
                        curl_code = result.returncode
                        if curl_code == 0:
                            curl_code = vm_result.returncode
                            
                        # Map common curl error codes to helpful messages
                        if curl_code == 7:
                            curl_error = "Failed to connect - API server is starting up"
                        elif curl_code == 22:
                            curl_error = "HTTP error returned from API server"
                        elif curl_code == 28:
                            curl_error = "Operation timeout - API server is slow to respond"
                        elif curl_code == 52:
                            curl_error = "Empty reply from server - API is starting but not ready"
                        elif curl_code == 56:
                            curl_error = "Network problem during data transfer"
                        else:
                            curl_error = f"Unknown curl error code: {curl_code}"
                            
                        logger.info(f"API not ready yet: {curl_error}")
            except subprocess.SubprocessError as e:
                logger.warning(f"Error checking API status: {e}")
                
            # If the container is running but API is not ready, that's OK - we'll just wait
            # a bit longer before checking again, as the container may still be initializing
            elapsed_seconds = time.time() - start_time
            if int(elapsed_seconds) % 5 == 0:  # Only print status every 5 seconds to reduce verbosity
                logger.debug(f"Waiting for API to initialize... ({elapsed_seconds:.1f}s / {timeout}s)")
            
            await asyncio.sleep(3)  # Longer sleep between API checks
        
        # Handle timeout - if the container is running but API is not ready, that's not
        # necessarily an error - the API might just need more time to start up
        if not container_running:
            logger.warning(f"Timed out waiting for container {container_name} to start")
            return False
        
        if not api_ready:
            logger.warning(f"Container {container_name} is running, but API is not fully ready yet.")
            logger.warning(f"NOTE: You may see some 'API request failed' messages while the API initializes.")
        
        # Return True if container is running, even if API isn't ready yet
        # This allows VM operations to proceed, with appropriate retries for API calls
        return container_running

    async def stop_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Stop a running VM by stopping the Lumier container."""
        try:
            # Use Docker commands to stop the container directly
            if hasattr(self, '_container_id') and self._container_id:
                logger.info(f"Stopping Lumier container: {self.container_name}")
                cmd = ["docker", "stop", self.container_name]
                result = subprocess.run(cmd, capture_output=True, text=True, check=True)
                logger.info(f"Container stopped: {result.stdout.strip()}")
                
                # Return minimal status info
                return {
                    "name": name,
                    "status": "stopped",
                    "container_id": self._container_id,
                }
            else:
                # Try to find the container by name
                check_cmd = ["docker", "ps", "-a", "--filter", f"name={self.container_name}", "--format", "{{.ID}}"]
                check_result = subprocess.run(check_cmd, capture_output=True, text=True)
                container_id = check_result.stdout.strip()
                
                if container_id:
                    logger.info(f"Found container ID: {container_id}")
                    cmd = ["docker", "stop", self.container_name]
                    result = subprocess.run(cmd, capture_output=True, text=True, check=True)
                    logger.info(f"Container stopped: {result.stdout.strip()}")
                    
                    return {
                        "name": name,
                        "status": "stopped",
                        "container_id": container_id,
                    }
                else:
                    logger.warning(f"No container found with name {self.container_name}")
                    return {
                        "name": name,
                        "status": "unknown",
                    }
        except subprocess.CalledProcessError as e:
            error_msg = f"Failed to stop container: {e.stderr if hasattr(e, 'stderr') else str(e)}"
            logger.error(error_msg)
            raise RuntimeError(f"Failed to stop Lumier container: {error_msg}")
            
    # update_vm is not implemented as it's not needed for Lumier
    # The BaseVMProvider requires it, so we provide a minimal implementation
    async def update_vm(self, name: str, update_opts: Dict[str, Any], storage: Optional[str] = None) -> Dict[str, Any]:
        """Not implemented for Lumier provider."""
        logger.warning("update_vm is not implemented for Lumier provider")
        return {"name": name, "status": "unchanged"}
        
    async def get_logs(self, name: str, num_lines: int = 100, follow: bool = False, timeout: Optional[int] = None) -> str:
        """Get the logs from the Lumier container.
        
        Args:
            name: Name of the VM/container to get logs for
            num_lines: Number of recent log lines to return (default: 100)
            follow: If True, follow the logs (stream new logs as they are generated)
            timeout: Optional timeout in seconds for follow mode (None means no timeout)
            
        Returns:
            Container logs as a string
            
        Note:
            If follow=True, this function will continuously stream logs until timeout
            or until interrupted. The output will be printed to console in real-time.
        """
        if not HAS_LUMIER:
            error_msg = "Docker is not available. Cannot get container logs."
            logger.error(error_msg)
            return error_msg
        
        # Make sure we have a container name
        container_name = name
        
        # Check if the container exists and is running
        try:
            # Check if the container exists
            inspect_cmd = ["docker", "container", "inspect", container_name]
            result = subprocess.run(inspect_cmd, capture_output=True, text=True)
            
            if result.returncode != 0:
                error_msg = f"Container '{container_name}' does not exist or is not accessible"
                logger.error(error_msg)
                return error_msg
        except Exception as e:
            error_msg = f"Error checking container status: {str(e)}"
            logger.error(error_msg)
            return error_msg
        
        # Base docker logs command
        log_cmd = ["docker", "logs"]
        
        # Add tail parameter to limit the number of lines
        log_cmd.extend(["--tail", str(num_lines)])
        
        # Handle follow mode with or without timeout
        if follow:
            log_cmd.append("--follow")
            
            if timeout is not None:
                # For follow mode with timeout, we'll run the command and handle the timeout
                log_cmd.append(container_name)
                logger.info(f"Following logs for container '{container_name}' with timeout {timeout}s")
                logger.info(f"\n---- CONTAINER LOGS FOR '{container_name}' (LIVE) ----")
                logger.info(f"Press Ctrl+C to stop following logs\n")
                
                try:
                    # Run with timeout
                    process = subprocess.Popen(log_cmd, text=True)
                    
                    # Wait for the specified timeout
                    if timeout:
                        try:
                            process.wait(timeout=timeout)
                        except subprocess.TimeoutExpired:
                            process.terminate()  # Stop after timeout
                            logger.info(f"\n---- LOG FOLLOWING STOPPED (timeout {timeout}s reached) ----")
                    else:
                        # Without timeout, wait for user interruption
                        process.wait()
                        
                    return "Logs were displayed to console in follow mode"
                except KeyboardInterrupt:
                    process.terminate()
                    logger.info("\n---- LOG FOLLOWING STOPPED (user interrupted) ----")
                    return "Logs were displayed to console in follow mode (interrupted)"
            else:
                # For follow mode without timeout, we'll print a helpful message
                log_cmd.append(container_name)
                logger.info(f"Following logs for container '{container_name}' indefinitely")
                logger.info(f"\n---- CONTAINER LOGS FOR '{container_name}' (LIVE) ----")
                logger.info(f"Press Ctrl+C to stop following logs\n")
                
                try:
                    # Run the command and let it run until interrupted
                    process = subprocess.Popen(log_cmd, text=True)
                    process.wait()  # Wait indefinitely (until user interrupts)
                    return "Logs were displayed to console in follow mode"
                except KeyboardInterrupt:
                    process.terminate()
                    logger.info("\n---- LOG FOLLOWING STOPPED (user interrupted) ----")
                    return "Logs were displayed to console in follow mode (interrupted)"
        else:
            # For non-follow mode, capture and return the logs as a string
            log_cmd.append(container_name)
            logger.info(f"Getting {num_lines} log lines for container '{container_name}'")
            
            try:
                result = subprocess.run(log_cmd, capture_output=True, text=True, check=True)
                logs = result.stdout
                
                # Only print header and logs if there's content
                if logs.strip():
                    logger.info(f"\n---- CONTAINER LOGS FOR '{container_name}' (LAST {num_lines} LINES) ----\n")
                    logger.info(logs)
                    logger.info(f"\n---- END OF LOGS ----")
                else:
                    logger.info(f"\nNo logs available for container '{container_name}'")
                    
                return logs
            except subprocess.CalledProcessError as e:
                error_msg = f"Error getting logs: {e.stderr}"
                logger.error(error_msg)
                return error_msg
            except Exception as e:
                error_msg = f"Unexpected error getting logs: {str(e)}"
                logger.error(error_msg)
                return error_msg
    
    async def get_ip(self, name: str, storage: Optional[str] = None, retry_delay: int = 2) -> str:
        """Get the IP address of a VM, waiting indefinitely until it's available.
        
        Args:
            name: Name of the VM to get the IP for
            storage: Optional storage path override
            retry_delay: Delay between retries in seconds (default: 2)
            
        Returns:
            IP address of the VM when it becomes available
        """
        # Use container_name = name for consistency
        self.container_name = name
        
        # Track total attempts for logging purposes
        total_attempts = 0
        
        # Loop indefinitely until we get a valid IP
        while True:
            total_attempts += 1
            
            # Log retry message but not on first attempt
            if total_attempts > 1:
                logger.info(f"Waiting for VM {name} IP address (attempt {total_attempts})...")
            
            try:
                # Get VM information
                vm_info = await self.get_vm(name, storage=storage)
                
                # Check if we got a valid IP
                ip = vm_info.get("ip_address", None)
                if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                    logger.info(f"Got valid VM IP address: {ip}")
                    return ip
                    
                # Check the VM status
                status = vm_info.get("status", "unknown")
                
                # Special handling for Lumier: it may report "stopped" even when the VM is starting
                # If the VM information contains an IP but status is stopped, it might be a race condition
                if status == "stopped" and "ip_address" in vm_info:
                    ip = vm_info.get("ip_address")
                    if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                        logger.info(f"Found valid IP {ip} despite VM status being {status}")
                        return ip
                    logger.info(f"VM status is {status}, but still waiting for IP to be assigned")
                # If VM is not running yet, log and wait
                elif status != "running":
                    logger.info(f"VM is not running yet (status: {status}). Waiting...")
                # If VM is running but no IP yet, wait and retry
                else:
                    logger.info("VM is running but no valid IP address yet. Waiting...")
                
            except Exception as e:
                logger.warning(f"Error getting VM {name} IP: {e}, continuing to wait...")
                
            # Wait before next retry
            await asyncio.sleep(retry_delay)
            
            # Add progress log every 10 attempts
            if total_attempts % 10 == 0:
                logger.info(f"Still waiting for VM {name} IP after {total_attempts} attempts...")
    
    async def __aenter__(self):
        """Async context manager entry.
        
        This method is called when entering an async context manager block.
        Returns self to be used in the context.
        """
        logger.debug("Entering LumierProvider context")
        
        # Initialize the API URL with the default value if not already set
        # This ensures get_vm can work before run_vm is called
        if not hasattr(self, '_api_url') or not self._api_url:
            self._api_url = f"http://{self.host}:{self.api_port}"
            logger.info(f"Initialized default Lumier API URL: {self._api_url}")
            
        return self
        
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit.
        
        This method is called when exiting an async context manager block.
        It handles proper cleanup of resources, including stopping any running containers.
        """
        logger.debug(f"Exiting LumierProvider context, handling exceptions: {exc_type}")
        try:
            # If we have a container ID, we should stop it to clean up resources
            if hasattr(self, '_container_id') and self._container_id:
                logger.info(f"Stopping Lumier container on context exit: {self.container_name}")
                try:
                    cmd = ["docker", "stop", self.container_name]
                    subprocess.run(cmd, capture_output=True, text=True, check=True)
                    logger.info(f"Container stopped during context exit: {self.container_name}")
                except subprocess.CalledProcessError as e:
                    logger.warning(f"Failed to stop container during cleanup: {e.stderr}")
                    # Don't raise an exception here, we want to continue with cleanup
        except Exception as e:
            logger.error(f"Error during LumierProvider cleanup: {e}")
            # We don't want to suppress the original exception if there was one
            if exc_type is None:
                raise
        # Return False to indicate that any exception should propagate
        return False
