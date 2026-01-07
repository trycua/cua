"""Async general getters - command execution, terminal output."""

import logging
from typing import Any, Dict, Optional

from .base import run_powershell

logger = logging.getLogger("winarena.getters_async.general")


async def get_vm_command_line(session, config: Dict[str, Any]) -> Optional[str]:
    """Execute a command and return stdout.

    Config:
        command (str): Command to execute
        shell (bool): Whether to use shell (default: False)

    Returns:
        Command output as string
    """
    command = config.get("command")
    if not command:
        return None

    result = await session.run_command(command)

    if hasattr(result, "stdout"):
        return result.stdout
    return str(result)


async def get_vm_command_error(session, config: Dict[str, Any]) -> Optional[str]:
    """Execute a command and return stderr.

    Config:
        command (str): Command to execute

    Returns:
        Command stderr as string
    """
    command = config.get("command")
    if not command:
        return None

    result = await session.run_command(command)

    if hasattr(result, "stderr"):
        return result.stderr
    return ""


async def get_vm_terminal_output(session, config: Dict[str, Any]) -> Optional[str]:
    """Get terminal output.

    Note: This is a simplified implementation. The original WAA used
    a persistent terminal session.

    Returns:
        Last terminal output or None
    """
    # For now, just return empty - would need terminal state tracking
    logger.warning("get_vm_terminal_output not fully implemented for async")
    return ""
