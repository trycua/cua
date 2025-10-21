"""
Command invoker for executing snapshot commands.
"""

from typing import Dict, Any, List
import logging

from .base import SnapshotCommand

logger = logging.getLogger(__name__)


class CommandInvoker:
    """
    Invoker for executing snapshot commands.
    Eliminates if-else statements by using polymorphism.
    """

    def __init__(self):
        """Initialize command invoker."""
        self.command_history: List[SnapshotCommand] = []

    async def execute_command(self, command: SnapshotCommand) -> Dict[str, Any]:
        """
        Execute a command if it can be executed.
        No if-else statements - delegates to command objects.

        Args:
            command: Command to execute

        Returns:
            Command execution result
        """
        try:
            can_execute = await command.can_execute()
            if not can_execute:
                return {
                    "status": "error",
                    "error": f"Command {command.get_command_name()} cannot be executed in current state"
                }

            logger.info(f"Executing command: {command.get_command_name()}")
            result = await command.execute()

            self.command_history.append(command)
            return result

        except Exception as e:
            error_msg = f"Failed to execute command {command.get_command_name()}: {str(e)}"
            logger.error(error_msg)
            return {"status": "error", "error": error_msg}

    async def execute_commands(self, commands: List[SnapshotCommand]) -> List[Dict[str, Any]]:
        """Execute multiple commands in sequence."""
        results = []
        for command in commands:
            result = await self.execute_command(command)
            results.append(result)
            # Stop on first error
            if result.get("status") == "error":
                break
        return results

    def get_command_history(self) -> List[str]:
        """Get history of executed commands."""
        return [cmd.get_command_name() for cmd in self.command_history]

    def clear_history(self) -> None:
        """Clear command history."""
        self.command_history.clear()