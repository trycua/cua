"""
Computer handler factory and interface definitions.

This module provides a factory function to create computer handlers from different
computer interface types, supporting the AsyncComputerHandler protocol, cua-sandbox
``Sandbox`` instances, and dicts of callables. To control the local machine,
use cua-driver (its SDK or MCP server).
"""

try:
    from cua_sandbox import Sandbox as cuaSandbox
except ImportError:
    cuaSandbox = None  # type: ignore[assignment,misc]

from .base import AsyncComputerHandler
from .custom import CustomComputerHandler
from .sandbox import SandboxComputerHandler


def _is_sandbox_like(computer) -> bool:
    return cuaSandbox is not None and isinstance(computer, cuaSandbox)


def is_agent_computer(computer):
    """Check if the given computer is a ComputerHandler, cua-sandbox Sandbox or dict."""
    return (
        isinstance(computer, AsyncComputerHandler)
        or _is_sandbox_like(computer)
        or isinstance(computer, dict)
    )


async def make_computer_handler(computer):
    """
    Create a computer handler from a computer interface.

    Args:
        computer: Either a ComputerHandler instance, a cua-sandbox ``Sandbox``
                  instance, or dict of functions

    Returns:
        ComputerHandler: A computer handler instance

    Raises:
        ValueError: If the computer type is not supported
    """
    if isinstance(computer, AsyncComputerHandler):
        return computer
    if _is_sandbox_like(computer):
        return SandboxComputerHandler(computer)
    if isinstance(computer, dict):
        return CustomComputerHandler(computer)
    raise ValueError(f"Unsupported computer type: {type(computer)}")
