"""Shared API utilities for Lume and Lumier providers.

This module contains shared functions for interacting with the Lume API,
used by both the LumeProvider and LumierProvider classes.
"""

import logging
import json
import subprocess
import urllib.parse
from typing import Dict, List, Optional, Any

# Setup logging
logger = logging.getLogger(__name__)

# Check if curl is available
try:
    subprocess.run(["curl", "--version"], capture_output=True, check=True)
    HAS_CURL = True
except (subprocess.SubprocessError, FileNotFoundError):
    HAS_CURL = False


def lume_api_get(
    vm_name: str,
    host: str,
    port: int,
    storage: Optional[str] = None,
    debug: bool = False,
    verbose: bool = False
) -> Dict[str, Any]:
    """Use curl to get VM information from Lume API.
    
    Args:
        vm_name: Name of the VM to get info for
        host: API host
        port: API port
        storage: Storage path for the VM
        debug: Whether to show debug output
        verbose: Enable verbose logging
        
    Returns:
        Dictionary with VM status information parsed from JSON response
    """
    # URL encode the storage parameter for the query
    encoded_storage = ""
    storage_param = ""
    
    if storage:
        # First encode the storage path properly
        encoded_storage = urllib.parse.quote(storage, safe='')
        storage_param = f"?storage={encoded_storage}"
        
    # Construct API URL with encoded storage parameter if needed
    api_url = f"http://{host}:{port}/lume/vms/{vm_name}{storage_param}"
        
    # Construct the curl command with increased timeouts for more reliability
    # --connect-timeout: Time to establish connection (15 seconds)
    # --max-time: Maximum time for the whole operation (20 seconds)
    # -f: Fail silently (no output at all) on server errors
    # Add single quotes around URL to ensure special characters are handled correctly
    cmd = ["curl", "--connect-timeout", "15", "--max-time", "20", "-s", "-f", f"'{api_url}'"]
    
    # For logging and display, show the properly escaped URL
    display_cmd = ["curl", "--connect-timeout", "15", "--max-time", "20", "-s", "-f", api_url]
    
    # Only print the curl command when debug is enabled
    display_curl_string = ' '.join(display_cmd)
    logger.debug(f"Executing API request: {display_curl_string}")
    
    # Execute the command - for execution we need to use shell=True to handle URLs with special characters
    try:
        # Use a single string with shell=True for proper URL handling
        shell_cmd = ' '.join(cmd)
        result = subprocess.run(shell_cmd, shell=True, capture_output=True, text=True)
        
        # Handle curl exit codes
        if result.returncode != 0:
            curl_error = "Unknown error"
            
            # Map common curl error codes to helpful messages
            if result.returncode == 7:
                curl_error = "Failed to connect to the API server - it might still be starting up"
            elif result.returncode == 22:
                curl_error = "HTTP error returned from API server"
            elif result.returncode == 28:
                curl_error = "Operation timeout - the API server is taking too long to respond"
            elif result.returncode == 52:
                curl_error = "Empty reply from server - the API server is starting but not fully ready yet"
            elif result.returncode == 56:
                curl_error = "Network problem during data transfer - check container networking"
                
            # Only log at debug level to reduce noise during retries
            logger.debug(f"API request failed with code {result.returncode}: {curl_error}")
            
            # Return a more useful error message
            return {
                "error": f"API request failed: {curl_error}",
                "curl_code": result.returncode,
                "vm_name": vm_name,
                "status": "unknown"  # We don't know the actual status due to API error
            }
            
        # Try to parse the response as JSON
        if result.stdout and result.stdout.strip():
            try:
                vm_status = json.loads(result.stdout)
                if debug or verbose:
                    logger.info(f"Successfully parsed VM status: {vm_status.get('status', 'unknown')}")
                return vm_status
            except json.JSONDecodeError as e:
                # Return the raw response if it's not valid JSON
                logger.warning(f"Invalid JSON response: {e}")
                if "Virtual machine not found" in result.stdout:
                    return {"status": "not_found", "message": "VM not found in Lume API"}
                
                return {"error": f"Invalid JSON response: {result.stdout[:100]}...", "status": "unknown"}
        else:
            return {"error": "Empty response from API", "status": "unknown"}
    except subprocess.SubprocessError as e:
        logger.error(f"Failed to execute API request: {e}")
        return {"error": f"Failed to execute API request: {str(e)}", "status": "unknown"}


def lume_api_run(
    vm_name: str,
    host: str,
    port: int,
    run_opts: Dict[str, Any],
    storage: Optional[str] = None,
    debug: bool = False,
    verbose: bool = False
) -> Dict[str, Any]:
    """Run a VM using curl.
    
    Args:
        vm_name: Name of the VM to run
        host: API host
        port: API port
        run_opts: Dictionary of run options
        storage: Storage path for the VM
        debug: Whether to show debug output
        verbose: Enable verbose logging
        
    Returns:
        Dictionary with API response or error information
    """
    # Construct API URL
    api_url = f"http://{host}:{port}/lume/vms/{vm_name}/run"
    
    # Prepare JSON payload with required parameters
    payload = {}
    
    # Add CPU cores if specified
    if "cpu" in run_opts:
        payload["cpu"] = run_opts["cpu"]
        
    # Add memory if specified
    if "memory" in run_opts:
        payload["memory"] = run_opts["memory"]
    
    # Add storage parameter if specified
    if storage:
        payload["storage"] = storage
    elif "storage" in run_opts:
        payload["storage"] = run_opts["storage"]
        
    # Add shared directories if specified
    if "shared_directories" in run_opts and run_opts["shared_directories"]:
        payload["sharedDirectories"] = run_opts["shared_directories"]
        
    # Log the payload for debugging
    logger.debug(f"API payload: {json.dumps(payload, indent=2)}")
    
    # Construct the curl command
    cmd = [
        "curl", "--connect-timeout", "30", "--max-time", "30",
        "-s", "-X", "POST", "-H", "Content-Type: application/json",
        "-d", json.dumps(payload),
        api_url
    ]
    
    # Execute the command
    try:
        result = subprocess.run(cmd, capture_output=True, text=True)
        
        if result.returncode != 0:
            logger.warning(f"API request failed with code {result.returncode}: {result.stderr}")
            return {"error": f"API request failed: {result.stderr}"}
            
        # Try to parse the response as JSON
        if result.stdout and result.stdout.strip():
            try:
                response = json.loads(result.stdout)
                return response
            except json.JSONDecodeError:
                # Return the raw response if it's not valid JSON
                return {"success": True, "message": "VM started successfully", "raw_response": result.stdout}
        else:
            return {"success": True, "message": "VM started successfully"}
    except subprocess.SubprocessError as e:
        logger.error(f"Failed to execute run request: {e}")
        return {"error": f"Failed to execute run request: {str(e)}"}


def lume_api_stop(
    vm_name: str,
    host: str,
    port: int,
    storage: Optional[str] = None,
    debug: bool = False,
    verbose: bool = False
) -> Dict[str, Any]:
    """Stop a VM using curl.
    
    Args:
        vm_name: Name of the VM to stop
        host: API host
        port: API port
        storage: Storage path for the VM
        debug: Whether to show debug output
        verbose: Enable verbose logging
        
    Returns:
        Dictionary with API response or error information
    """
    # Construct API URL
    api_url = f"http://{host}:{port}/lume/vms/{vm_name}/stop"
    
    # Prepare JSON payload with required parameters
    payload = {}
    
    # Add storage path if specified
    if storage:
        payload["storage"] = storage
        
    # Construct the curl command
    cmd = [
        "curl", "--connect-timeout", "15", "--max-time", "20",
        "-s", "-X", "POST", "-H", "Content-Type: application/json",
        "-d", json.dumps(payload),
        api_url
    ]
    
    # Execute the command
    try:
        if debug or verbose:
            logger.info(f"Executing: {' '.join(cmd)}")
            
        result = subprocess.run(cmd, capture_output=True, text=True)
        
        if result.returncode != 0:
            logger.warning(f"API request failed with code {result.returncode}: {result.stderr}")
            return {"error": f"API request failed: {result.stderr}"}
            
        # Try to parse the response as JSON
        if result.stdout and result.stdout.strip():
            try:
                response = json.loads(result.stdout)
                return response
            except json.JSONDecodeError:
                # Return the raw response if it's not valid JSON
                return {"success": True, "message": "VM stopped successfully", "raw_response": result.stdout}
        else:
            return {"success": True, "message": "VM stopped successfully"}
    except subprocess.SubprocessError as e:
        logger.error(f"Failed to execute stop request: {e}")
        return {"error": f"Failed to execute stop request: {str(e)}"}


def lume_api_update(
    vm_name: str,
    host: str,
    port: int,
    update_opts: Dict[str, Any],
    storage: Optional[str] = None,
    debug: bool = False,
    verbose: bool = False
) -> Dict[str, Any]:
    """Update VM settings using curl.
    
    Args:
        vm_name: Name of the VM to update
        host: API host
        port: API port
        update_opts: Dictionary of update options
        storage: Storage path for the VM
        debug: Whether to show debug output
        verbose: Enable verbose logging
        
    Returns:
        Dictionary with API response or error information
    """
    # Construct API URL
    api_url = f"http://{host}:{port}/lume/vms/{vm_name}/update"
    
    # Prepare JSON payload with required parameters
    payload = {}
    
    # Add CPU cores if specified
    if "cpu" in update_opts:
        payload["cpu"] = update_opts["cpu"]
        
    # Add memory if specified
    if "memory" in update_opts:
        payload["memory"] = update_opts["memory"]
    
    # Add storage path if specified
    if storage:
        payload["storage"] = storage
        
    # Construct the curl command
    cmd = [
        "curl", "--connect-timeout", "15", "--max-time", "20",
        "-s", "-X", "POST", "-H", "Content-Type: application/json",
        "-d", json.dumps(payload),
        api_url
    ]
    
    # Execute the command
    try:
        if debug:
            logger.info(f"Executing: {' '.join(cmd)}")
            
        result = subprocess.run(cmd, capture_output=True, text=True)
        
        if result.returncode != 0:
            logger.warning(f"API request failed with code {result.returncode}: {result.stderr}")
            return {"error": f"API request failed: {result.stderr}"}
            
        # Try to parse the response as JSON
        if result.stdout and result.stdout.strip():
            try:
                response = json.loads(result.stdout)
                return response
            except json.JSONDecodeError:
                # Return the raw response if it's not valid JSON
                return {"success": True, "message": "VM updated successfully", "raw_response": result.stdout}
        else:
            return {"success": True, "message": "VM updated successfully"}
    except subprocess.SubprocessError as e:
        logger.error(f"Failed to execute update request: {e}")
        return {"error": f"Failed to execute update request: {str(e)}"}


def lume_api_pull(
    image: str,
    name: str,
    host: str,
    port: int,
    storage: Optional[str] = None,
    registry: str = "ghcr.io",
    organization: str = "trycua",
    debug: bool = False,
    verbose: bool = False
) -> Dict[str, Any]:
    """Pull a VM image from a registry using curl.
    
    Args:
        image: Name/tag of the image to pull
        name: Name to give the VM after pulling
        host: API host
        port: API port
        storage: Storage path for the VM
        registry: Registry to pull from (default: ghcr.io)
        organization: Organization in registry (default: trycua)
        debug: Whether to show debug output
        verbose: Enable verbose logging
        
    Returns:
        Dictionary with pull status and information
    """
    # Prepare pull request payload
    pull_payload = {
        "image": image,  # Use provided image name
        "name": name, # Always use name as the target VM name
        "registry": registry,
        "organization": organization
    }
    
    if storage:
        pull_payload["storage"] = storage
    
    # Construct pull command with proper JSON payload
    pull_cmd = [
        "curl"
    ]
    
    if not verbose:
        pull_cmd.append("-s")
    
    pull_cmd.extend([
        "-X", "POST",
        "-H", "Content-Type: application/json",
        "-d", json.dumps(pull_payload),
        f"http://{host}:{port}/lume/pull"
    ])
    
    logger.debug(f"Executing API request: {' '.join(pull_cmd)}")
    
    try:
        # Execute pull command
        result = subprocess.run(pull_cmd, capture_output=True, text=True)
        
        if result.returncode != 0:
            error_msg = f"Failed to pull VM {name}: {result.stderr}"
            logger.error(error_msg)
            return {"error": error_msg}
        
        try:
            response = json.loads(result.stdout)
            logger.info(f"Successfully initiated pull for VM {name}")
            return response
        except json.JSONDecodeError:
            if result.stdout:
                logger.info(f"Pull response: {result.stdout}")
            return {"success": True, "message": f"Successfully initiated pull for VM {name}"}
            
    except subprocess.SubprocessError as e:
        error_msg = f"Failed to execute pull command: {str(e)}"
        logger.error(error_msg)
        return {"error": error_msg}


def lume_api_delete(
    vm_name: str,
    host: str,
    port: int,
    storage: Optional[str] = None,
    debug: bool = False,
    verbose: bool = False
) -> Dict[str, Any]:
    """Delete a VM using curl.
    
    Args:
        vm_name: Name of the VM to delete
        host: API host
        port: API port
        storage: Storage path for the VM
        debug: Whether to show debug output
        verbose: Enable verbose logging
        
    Returns:
        Dictionary with API response or error information
    """
    # URL encode the storage parameter for the query
    encoded_storage = ""
    storage_param = ""
    
    if storage:
        # First encode the storage path properly
        encoded_storage = urllib.parse.quote(storage, safe='')
        storage_param = f"?storage={encoded_storage}"
        
    # Construct API URL with encoded storage parameter if needed
    api_url = f"http://{host}:{port}/lume/vms/{vm_name}{storage_param}"
        
    # Construct the curl command for DELETE operation - using much longer timeouts matching shell implementation
    cmd = ["curl", "--connect-timeout", "6000", "--max-time", "5000", "-s", "-X", "DELETE", f"'{api_url}'"]
    
    # For logging and display, show the properly escaped URL
    display_cmd = ["curl", "--connect-timeout", "6000", "--max-time", "5000", "-s", "-X", "DELETE", api_url]
    
    # Only print the curl command when debug is enabled
    display_curl_string = ' '.join(display_cmd)
    logger.debug(f"Executing API request: {display_curl_string}")
    
    # Execute the command - for execution we need to use shell=True to handle URLs with special characters
    try:
        # Use a single string with shell=True for proper URL handling
        shell_cmd = ' '.join(cmd)
        result = subprocess.run(shell_cmd, shell=True, capture_output=True, text=True)
        
        # Handle curl exit codes
        if result.returncode != 0:
            curl_error = "Unknown error"
            
            # Map common curl error codes to helpful messages
            if result.returncode == 7:
                curl_error = "Failed to connect to the API server - it might still be starting up"
            elif result.returncode == 22:
                curl_error = "HTTP error returned from API server"
            elif result.returncode == 28:
                curl_error = "Operation timeout - the API server is taking too long to respond"
            elif result.returncode == 52:
                curl_error = "Empty reply from server - the API server is starting but not fully ready yet"
            elif result.returncode == 56:
                curl_error = "Network problem during data transfer - check container networking"
                
            # Only log at debug level to reduce noise during retries
            logger.debug(f"API request failed with code {result.returncode}: {curl_error}")
            
            # Return a more useful error message
            return {
                "error": f"API request failed: {curl_error}",
                "curl_code": result.returncode,
                "vm_name": vm_name,
                "storage": storage
            }
            
        # Try to parse the response as JSON
        if result.stdout and result.stdout.strip():
            try:
                response = json.loads(result.stdout)
                return response
            except json.JSONDecodeError:
                # Return the raw response if it's not valid JSON
                return {"success": True, "message": "VM deleted successfully", "raw_response": result.stdout}
        else:
            return {"success": True, "message": "VM deleted successfully"}
    except subprocess.SubprocessError as e:
        logger.error(f"Failed to execute delete request: {e}")
        return {"error": f"Failed to execute delete request: {str(e)}"}


def parse_memory(memory_str: str) -> int:
    """Parse memory string to MB integer.
    
    Examples:
        "8GB" -> 8192
        "1024MB" -> 1024
        "512" -> 512
        
    Returns:
        Memory value in MB
    """
    if isinstance(memory_str, int):
        return memory_str
        
    if isinstance(memory_str, str):
        # Extract number and unit
        import re
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
