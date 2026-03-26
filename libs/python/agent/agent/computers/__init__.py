"""
Computer handler factory and interface definitions.

This module provides a factory function to create computer handlers from different
computer interface types, supporting both the ComputerHandler protocol and the
Computer library interface.
"""

try:
    from computer import Computer as cuaComputer

    from .cua import cuaComputerHandler
except ImportError:
    cuaComputer = None  # type: ignore[assignment,misc]
    cuaComputerHandler = None  # type: ignore[assignment]

try:
    from cua_sandbox import Sandbox as cuaSandbox
except ImportError:
    cuaSandbox = None  # type: ignore[assignment,misc]

from .base import AsyncComputerHandler
from .custom import CustomComputerHandler
from .sandbox import SandboxComputerHandler


def is_agent_computer(computer):
    """Check if the given computer is a ComputerHandler or Cua Computer."""
    return (
        isinstance(computer, AsyncComputerHandler)
        or (cuaComputer is not None and isinstance(computer, cuaComputer))
        or (cuaSandbox is not None and isinstance(computer, cuaSandbox))
        or (isinstance(computer, dict))
    )  # and "screenshot" in computer)


async def make_computer_handler(computer):
    """
    Create a computer handler from a computer interface.

    Args:
        computer: Either a ComputerHandler instance, Computer instance,
                  Sandbox instance, or dict of functions

    Returns:
        ComputerHandler: A computer handler instance

    Raises:
        ValueError: If the computer type is not supported
    """
    if isinstance(computer, AsyncComputerHandler):
        return computer
    if cuaComputer is not None and isinstance(computer, cuaComputer):
        computer_handler = cuaComputerHandler(computer)
        await computer_handler._initialize()
        return computer_handler
    if cuaSandbox is not None and isinstance(computer, cuaSandbox):
        return SandboxComputerHandler(computer)
    if isinstance(computer, dict):
        return CustomComputerHandler(computer)
    raise ValueError(f"Unsupported computer type: {type(computer)}")
