"""
Base command interface for snapshot operations.
"""

from abc import ABC, abstractmethod
from typing import Dict, Any, List


class SnapshotCommand(ABC):
    """
    Abstract base class for snapshot commands.
    Eliminates if-else statements by encapsulating operations.
    """

    @abstractmethod
    async def execute(self) -> Dict[str, Any]:
        """Execute the command."""
        pass

    @abstractmethod
    async def can_execute(self) -> bool:
        """Check if the command can be executed."""
        pass

    @abstractmethod
    def get_command_name(self) -> str:
        """Get the name of this command."""
        pass

    def get_command_info(self) -> Dict[str, Any]:
        """Get information about this command."""
        return {
            "command": self.get_command_name(),
            "can_execute": False  # Will be updated by async call
        }