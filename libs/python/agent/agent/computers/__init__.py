"""
Computer handler factory and interface definitions.

This module provides a factory function to create computer handlers from different
computer interface types, supporting both the ComputerHandler protocol and the
Computer library interface.
"""

from .base import AsyncComputerHandler
from .cua import cuaComputerHandler
from .custom import CustomComputerHandler
from computer import Computer as cuaComputer

def is_agent_computer(computer):
    """Check if the given computer is a ComputerHandler or CUA Computer."""
    return isinstance(computer, AsyncComputerHandler) or \
        isinstance(computer, cuaComputer) or \
        (isinstance(computer, dict)) #and "screenshot" in computer)

async def make_computer_handler(computer):
    """
    Create a computer handler from a computer interface.
    
    Args:
        computer: Either a ComputerHandler instance, Computer instance, or dict of functions
        
    Returns:
        ComputerHandler: A computer handler instance
        
    Raises:
        ValueError: If the computer type is not supported
    """
    if isinstance(computer, AsyncComputerHandler):
        return computer
    if isinstance(computer, cuaComputer):
        computer_handler = cuaComputerHandler(computer)
        await computer_handler._initialize()
        return computer_handler
    if isinstance(computer, dict):
        return CustomComputerHandler(computer)
    raise ValueError(f"Unsupported computer type: {type(computer)}")