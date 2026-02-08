"""Abstract base bash/shell tool implementation."""

import asyncio
import logging
from abc import abstractmethod
from typing import Any, Dict, Tuple

from computer.computer import Computer

from .base import BaseTool, ToolResult


class BaseBashTool(BaseTool):
    """Base class for bash/shell command execution tools across different providers."""

    name = "bash"
    logger = logging.getLogger(__name__)
    computer: Computer

    def __init__(self, computer: Computer):
        """Initialize the BashTool.

        Args:
            computer: Computer instance, may be used for related operations
        """
        self.computer = computer

    async def run_command(self, command: str) -> Tuple[int, str, str]:
        """Run a shell command and return exit code, stdout, and stderr.

        Args:
            command: Shell command to execute

        Returns:
            Tuple containing (exit_code, stdout, stderr)
        """
        try:
            process = await asyncio.create_subprocess_shell(
                command,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await process.communicate()
            return process.returncode or 0, stdout.decode(), stderr.decode()
        except Exception as e:
            self.logger.error(f"Error running command: {str(e)}")
            return 1, "", str(e)

    @abstractmethod
    async def __call__(self, **kwargs) -> ToolResult:
        """Execute the tool with the provided arguments."""
        raise NotImplementedError
