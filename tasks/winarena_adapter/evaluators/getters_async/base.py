"""Base utilities for async getters.

All async getters receive a `session` object (cua-bench DesktopSession)
and a `config` dict with getter-specific parameters.

The session provides async methods that make HTTP calls to cua-computer-server:
- session.run_command(cmd) -> CommandResult
- session.read_bytes(path) -> bytes
- session.read_text(path) -> str
- session.file_exists(path) -> bool
- session.get_accessibility_tree() -> dict
- etc.
"""

import logging
import os
import tempfile
from pathlib import Path
from typing import Any, Dict, Optional

logger = logging.getLogger("winarena.getters_async")

# Cache directory for downloaded files
CACHE_DIR = Path(tempfile.gettempdir()) / "waa_cache"
CACHE_DIR.mkdir(parents=True, exist_ok=True)


def get_cache_path(filename: str) -> Path:
    """Get a cache file path."""
    return CACHE_DIR / filename


async def run_powershell(session, script: str) -> Dict[str, Any]:
    """Run a PowerShell command and return the result.

    Args:
        session: cua-bench DesktopSession
        script: PowerShell script to run

    Returns:
        Dict with 'stdout', 'stderr', 'exit_code'
    """
    # Escape quotes in script
    escaped = script.replace('"', '\\"')
    cmd = f'powershell -Command "{escaped}"'

    result = await session.run_command(cmd)

    return {
        "stdout": result.stdout if hasattr(result, "stdout") else str(result),
        "stderr": result.stderr if hasattr(result, "stderr") else "",
        "exit_code": result.exit_code if hasattr(result, "exit_code") else 0,
    }


async def run_python(session, code: str) -> Dict[str, Any]:
    """Run Python code on the VM and return the result.

    Args:
        session: cua-bench DesktopSession
        code: Python code to execute

    Returns:
        Dict with 'stdout', 'stderr', 'exit_code'
    """
    # Escape quotes and newlines
    escaped = code.replace('"', '\\"').replace('\n', '; ')
    cmd = f'python -c "{escaped}"'

    result = await session.run_command(cmd)

    return {
        "stdout": result.stdout if hasattr(result, "stdout") else str(result),
        "stderr": result.stderr if hasattr(result, "stderr") else "",
        "exit_code": result.exit_code if hasattr(result, "exit_code") else 0,
    }


async def get_registry_value(session, key_path: str, value_name: str) -> Optional[str]:
    """Get a Windows registry value.

    Args:
        session: cua-bench DesktopSession
        key_path: Registry key path (e.g., "HKCU:\\Software\\...")
        value_name: Name of the value to retrieve

    Returns:
        The value as a string, or None if not found
    """
    result = await run_powershell(
        session,
        f"Get-ItemPropertyValue -Path '{key_path}' -Name '{value_name}'"
    )

    if result["exit_code"] == 0 and result["stdout"]:
        return result["stdout"].strip()
    return None


async def get_registry_binary(session, key_path: str, value_name: str) -> Optional[bytes]:
    """Get a Windows registry binary value.

    Args:
        session: cua-bench DesktopSession
        key_path: Registry key path
        value_name: Name of the value to retrieve

    Returns:
        The binary value as bytes, or None if not found
    """
    # Get binary value as comma-separated bytes
    result = await run_powershell(
        session,
        f"(Get-ItemProperty -Path '{key_path}' -Name '{value_name}').{value_name} -join ','"
    )

    if result["exit_code"] == 0 and result["stdout"]:
        try:
            byte_values = [int(b) for b in result["stdout"].strip().split(",")]
            return bytes(byte_values)
        except (ValueError, TypeError):
            return None
    return None


async def download_file_from_vm(session, vm_path: str, local_dest: str) -> Optional[str]:
    """Download a file from the VM to local cache.

    Args:
        session: cua-bench DesktopSession
        vm_path: Path to file on VM
        local_dest: Filename in cache directory

    Returns:
        Local path to downloaded file, or None if failed
    """
    try:
        content = await session.read_bytes(vm_path)
        if content is None:
            return None

        local_path = get_cache_path(local_dest)
        local_path.write_bytes(content)
        return str(local_path)
    except Exception as e:
        logger.error(f"Failed to download {vm_path}: {e}")
        return None
