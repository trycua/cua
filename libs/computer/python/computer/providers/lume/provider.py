"""Lume VM provider implementation using curl commands.

This provider uses direct curl commands to interact with the Lume API,
removing the dependency on the pylume Python package.
"""

import os
import re
import asyncio
import json
import logging
import subprocess
import urllib.parse
from typing import Dict, Any, Optional, List, Tuple

from ..base import BaseVMProvider, VMProviderType
from ...logger import Logger, LogLevel
from ..lume_api import (
    lume_api_get,
    lume_api_run,
    lume_api_stop,
    lume_api_update,
    lume_api_pull,
    HAS_CURL,
    parse_memory
)

# Setup logging
logger = logging.getLogger(__name__)

class LumeProvider(BaseVMProvider):
    """Lume VM provider implementation using direct curl commands.
    
    This provider uses curl to interact with the Lume API server,
    removing the dependency on the pylume Python package.
    """
    
    def __init__(
        self, 
        port: int = 7777,
        host: str = "localhost",
        storage: Optional[str] = None,
        verbose: bool = False,
        ephemeral: bool = False,
    ):
        """Initialize the Lume provider.
        
        Args:
            port: Port for the Lume API server (default: 7777)
            host: Host to use for API connections (default: localhost)
            storage: Path to store VM data
            verbose: Enable verbose logging
        """
        if not HAS_CURL:
            raise ImportError(
                "curl is required for LumeProvider. "
                "Please ensure it is installed and in your PATH."
            )
            
        self.host = host
        self.port = port  # Default port for Lume API
        self.storage = storage
        self.verbose = verbose
        self.ephemeral = ephemeral  # If True, VMs will be deleted after stopping
        
        # Base API URL for Lume API calls
        self.api_base_url = f"http://{self.host}:{self.port}"
        
        self.logger = logging.getLogger(__name__)
        
    @property
    def provider_type(self) -> VMProviderType:
        """Get the provider type."""
        return VMProviderType.LUME
        
    async def __aenter__(self):
        """Enter async context manager."""
        # No initialization needed, just return self
        return self
        
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Exit async context manager."""
        # No cleanup needed
        pass
            
    def _lume_api_get(self, vm_name: str = "", storage: Optional[str] = None, debug: bool = False) -> Dict[str, Any]:
        """Get VM information using shared lume_api function.
        
        Args:
            vm_name: Optional name of the VM to get info for.
                     If empty, lists all VMs.
            storage: Optional storage path override. If provided, this will be used instead of self.storage
            debug: Whether to show debug output
            
        Returns:
            Dictionary with VM status information parsed from JSON response
        """
        # Use the shared implementation from lume_api module
        return lume_api_get(
            vm_name=vm_name,
            host=self.host,
            port=self.port,
            storage=storage if storage is not None else self.storage,
            debug=debug,
            verbose=self.verbose
        )
    
    def _lume_api_run(self, vm_name: str, run_opts: Dict[str, Any], debug: bool = False) -> Dict[str, Any]:
        """Run a VM using shared lume_api function.
        
        Args:
            vm_name: Name of the VM to run
            run_opts: Dictionary of run options
            debug: Whether to show debug output
            
        Returns:
            Dictionary with API response or error information
        """
        # Use the shared implementation from lume_api module
        return lume_api_run(
            vm_name=vm_name, 
            host=self.host,
            port=self.port,
            run_opts=run_opts,
            storage=self.storage,
            debug=debug,
            verbose=self.verbose
        )
    
    def _lume_api_stop(self, vm_name: str, debug: bool = False) -> Dict[str, Any]:
        """Stop a VM using shared lume_api function.
        
        Args:
            vm_name: Name of the VM to stop
            debug: Whether to show debug output
            
        Returns:
            Dictionary with API response or error information
        """
        # Use the shared implementation from lume_api module
        return lume_api_stop(
            vm_name=vm_name, 
            host=self.host,
            port=self.port,
            storage=self.storage,
            debug=debug,
            verbose=self.verbose
        )
    
    def _lume_api_update(self, vm_name: str, update_opts: Dict[str, Any], debug: bool = False) -> Dict[str, Any]:
        """Update VM configuration using shared lume_api function.
        
        Args:
            vm_name: Name of the VM to update
            update_opts: Dictionary of update options
            debug: Whether to show debug output
            
        Returns:
            Dictionary with API response or error information
        """
        # Use the shared implementation from lume_api module
        return lume_api_update(
            vm_name=vm_name, 
            host=self.host,
            port=self.port,
            update_opts=update_opts,
            storage=self.storage,
            debug=debug,
            verbose=self.verbose
        )
    
    async def get_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Get VM information by name.
        
        Args:
            name: Name of the VM to get information for
            storage: Optional storage path override. If provided, this will be used
                    instead of the provider's default storage path.
            
        Returns:
            Dictionary with VM information including status, IP address, etc.
            
        Note:
            If storage is not provided, the provider's default storage path will be used.
            The storage parameter allows overriding the storage location for this specific call.
        """
        if not HAS_CURL:
            logger.error("curl is not available. Cannot get VM status.")
            return {
                "name": name,
                "status": "unavailable",
                "error": "curl is not available"
            }
        
        # First try to get detailed VM info from the API
        try:
            # Query the Lume API for VM status using the provider's storage_path
            vm_info = self._lume_api_get(
                vm_name=name, 
                storage=storage if storage is not None else self.storage,
                debug=self.verbose
            )
            
            # Check for API errors
            if "error" in vm_info:
                logger.debug(f"API request error: {vm_info['error']}")
                # If we got an error from the API, report the VM as not ready yet
                return {
                    "name": name,
                    "status": "starting",  # VM is still starting - do not attempt to connect yet
                    "api_status": "error",
                    "error": vm_info["error"]
                }
            
            # Process the VM status information
            vm_status = vm_info.get("status", "unknown")
            
            # Check if VM is stopped or not running - don't wait for IP in this case
            if vm_status == "stopped":
                logger.info(f"VM {name} is in '{vm_status}' state - not waiting for IP address")
                # Return the status as-is without waiting for an IP
                result = {
                    "name": name,
                    "status": vm_status,
                    **vm_info  # Include all original fields from the API response
                }
                return result
            
            # Handle field name differences between APIs
            # Some APIs use camelCase, others use snake_case
            if "vncUrl" in vm_info:
                vnc_url = vm_info["vncUrl"]
            elif "vnc_url" in vm_info:
                vnc_url = vm_info["vnc_url"]
            else:
                vnc_url = ""
                
            if "ipAddress" in vm_info:
                ip_address = vm_info["ipAddress"]
            elif "ip_address" in vm_info:
                ip_address = vm_info["ip_address"]
            else:
                # If no IP address is provided and VM is supposed to be running,
                # report it as still starting
                ip_address = None
                logger.info(f"VM {name} is in '{vm_status}' state but no IP address found - reporting as still starting")
                
            logger.info(f"VM {name} status: {vm_status}")
            
            # Return the complete status information
            result = {
                "name": name,
                "status": vm_status if vm_status else "running",
                "ip_address": ip_address,
                "vnc_url": vnc_url,
                "api_status": "ok"
            }
            
            # Include all original fields from the API response
            if isinstance(vm_info, dict):
                for key, value in vm_info.items():
                    if key not in result:  # Don't override our carefully processed fields
                        result[key] = value
                        
            return result
            
        except Exception as e:
            logger.error(f"Failed to get VM status: {e}")
            # Return a fallback status that indicates the VM is not ready yet
            return {
                "name": name,
                "status": "initializing",  # VM is still initializing
                "error": f"Failed to get VM status: {str(e)}"
            }
        
    async def list_vms(self) -> List[Dict[str, Any]]:
        """List all available VMs."""
        result = self._lume_api_get(debug=self.verbose)
        
        # Extract the VMs list from the response
        if "vms" in result and isinstance(result["vms"], list):
            return result["vms"]
        elif "error" in result:
            logger.error(f"Error listing VMs: {result['error']}")
            return []
        else:
            return []
        
    async def run_vm(self, image: str, name: str, run_opts: Dict[str, Any], storage: Optional[str] = None) -> Dict[str, Any]:
        """Run a VM with the given options.
        
        If the VM does not exist in the storage location, this will attempt to pull it
        from the Lume registry first.
        
        Args:
            image: Image name to use when pulling the VM if it doesn't exist
            name: Name of the VM to run
            run_opts: Dictionary of run options (memory, cpu, etc.)
            storage: Optional storage path override. If provided, this will be used
                    instead of the provider's default storage path.
        
        Returns:
            Dictionary with VM run status and information
        """
        # First check if VM exists by trying to get its info
        vm_info = await self.get_vm(name, storage=storage)
        
        if "error" in vm_info:
            # VM doesn't exist, try to pull it
            self.logger.info(f"VM {name} not found, attempting to pull image {image} from registry...")
            
            # Call pull_vm with the image parameter
            pull_result = await self.pull_vm(
                name=name, 
                image=image, 
                storage=storage
            )
            
            # Check if pull was successful
            if "error" in pull_result:
                self.logger.error(f"Failed to pull VM image: {pull_result['error']}")
                return pull_result  # Return the error from pull
                
            self.logger.info(f"Successfully pulled VM image {image} as {name}")
        
        # Now run the VM with the given options
        self.logger.info(f"Running VM {name} with options: {run_opts}")
        
        from ..lume_api import lume_api_run
        return lume_api_run(
            vm_name=name,
            host=self.host,
            port=self.port,
            run_opts=run_opts,
            storage=storage if storage is not None else self.storage,
            debug=self.verbose,
            verbose=self.verbose
        )
        
    async def stop_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Stop a running VM.
        
        If this provider was initialized with ephemeral=True, the VM will also
        be deleted after it is stopped.
        
        Args:
            name: Name of the VM to stop
            storage: Optional storage path override
            
        Returns:
            Dictionary with stop status and information
        """
        # Stop the VM first
        stop_result = self._lume_api_stop(name, debug=self.verbose)
        
        # Log ephemeral status for debugging
        self.logger.info(f"Ephemeral mode status: {self.ephemeral}")
        
        # If ephemeral mode is enabled, delete the VM after stopping
        if self.ephemeral and (stop_result.get("success", False) or "error" not in stop_result):
            self.logger.info(f"Ephemeral mode enabled - deleting VM {name} after stopping")
            try:
                delete_result = await self.delete_vm(name, storage=storage)
                
                # Return combined result
                return {
                    **stop_result,  # Include all stop result info
                    "deleted": True,
                    "delete_result": delete_result
                }
            except Exception as e:
                self.logger.error(f"Failed to delete ephemeral VM {name}: {e}")
                # Include the error but still return stop result
                return {
                    **stop_result,
                    "deleted": False,
                    "delete_error": str(e)
                }
        
        # Just return the stop result if not ephemeral
        return stop_result
        
    async def pull_vm(
        self,
        name: str,
        image: str,
        storage: Optional[str] = None,
        registry: str = "ghcr.io",
        organization: str = "trycua",
        pull_opts: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        """Pull a VM image from the registry.
        
        Args:
            name: Name for the VM after pulling
            image: The image name to pull (e.g. 'macos-sequoia-cua:latest')
            storage: Optional storage path to use
            registry: Registry to pull from (default: ghcr.io)
            organization: Organization in registry (default: trycua)
            pull_opts: Additional options for pulling the VM (optional)
            
        Returns:
            Dictionary with information about the pulled VM
            
        Raises:
            RuntimeError: If pull operation fails or image is not provided
        """
        # Validate image parameter
        if not image:
            raise ValueError("Image parameter is required for pull_vm")
            
        self.logger.info(f"Pulling VM image '{image}' as '{name}'")
        self.logger.info("You can check the pull progress using: lume logs -f")
        
        # Set default pull_opts if not provided
        if pull_opts is None:
            pull_opts = {}
            
        # Log information about the operation
        self.logger.debug(f"Pull storage location: {storage or 'default'}")
        
        try:
            # Call the lume_api_pull function from lume_api.py
            from ..lume_api import lume_api_pull
            
            result = lume_api_pull(
                image=image,
                name=name,
                host=self.host,
                port=self.port,
                storage=storage if storage is not None else self.storage,
                registry=registry,
                organization=organization,
                debug=self.verbose,
                verbose=self.verbose
            )
            
            # Check for errors in the result
            if "error" in result:
                self.logger.error(f"Failed to pull VM image: {result['error']}")
                return result
                
            self.logger.info(f"Successfully pulled VM image '{image}' as '{name}'")
            return result
        except Exception as e:
            self.logger.error(f"Failed to pull VM image '{image}': {e}")
            return {"error": f"Failed to pull VM: {str(e)}"}
        
    async def delete_vm(self, name: str, storage: Optional[str] = None) -> Dict[str, Any]:
        """Delete a VM permanently.
        
        Args:
            name: Name of the VM to delete
            storage: Optional storage path override
            
        Returns:
            Dictionary with delete status and information
        """
        self.logger.info(f"Deleting VM {name}...")
        
        try:
            # Call the lume_api_delete function we created
            from ..lume_api import lume_api_delete
            
            result = lume_api_delete(
                vm_name=name,
                host=self.host,
                port=self.port,
                storage=storage if storage is not None else self.storage,
                debug=self.verbose,
                verbose=self.verbose
            )
            
            # Check for errors in the result
            if "error" in result:
                self.logger.error(f"Failed to delete VM: {result['error']}")
                return result
                
            self.logger.info(f"Successfully deleted VM '{name}'")
            return result
        except Exception as e:
            self.logger.error(f"Failed to delete VM '{name}': {e}")
            return {"error": f"Failed to delete VM: {str(e)}"}
    
    async def update_vm(self, name: str, update_opts: Dict[str, Any], storage: Optional[str] = None) -> Dict[str, Any]:
        """Update VM configuration."""
        return self._lume_api_update(name, update_opts, debug=self.verbose)
        
    async def get_ip(self, name: str, storage: Optional[str] = None, retry_delay: int = 2) -> str:
        """Get the IP address of a VM, waiting indefinitely until it's available.
        
        Args:
            name: Name of the VM to get the IP for
            storage: Optional storage path override
            retry_delay: Delay between retries in seconds (default: 2)
            
        Returns:
            IP address of the VM when it becomes available
        """
        # Track total attempts for logging purposes
        total_attempts = 0
        
        # Loop indefinitely until we get a valid IP
        while True:
            total_attempts += 1
            
            # Log retry message but not on first attempt
            if total_attempts > 1:
                self.logger.info(f"Waiting for VM {name} IP address (attempt {total_attempts})...")
            
            try:
                # Get VM information
                vm_info = await self.get_vm(name, storage=storage)
                
                # Check if we got a valid IP
                ip = vm_info.get("ip_address", None)
                if ip and ip != "unknown" and not ip.startswith("0.0.0.0"):
                    self.logger.info(f"Got valid VM IP address: {ip}")
                    return ip
                    
                # Check the VM status
                status = vm_info.get("status", "unknown")
                
                # If VM is not running yet, log and wait
                if status != "running":
                    self.logger.info(f"VM is not running yet (status: {status}). Waiting...")
                # If VM is running but no IP yet, wait and retry
                else:
                    self.logger.info("VM is running but no valid IP address yet. Waiting...")
                
            except Exception as e:
                self.logger.warning(f"Error getting VM {name} IP: {e}, continuing to wait...")
                
            # Wait before next retry
            await asyncio.sleep(retry_delay)
            
            # Add progress log every 10 attempts
            if total_attempts % 10 == 0:
                self.logger.info(f"Still waiting for VM {name} IP after {total_attempts} attempts...")
        

