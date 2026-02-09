"""Abstract base edit tool implementation."""

import asyncio
import logging
import os
from abc import abstractmethod
from pathlib import Path
from typing import Any, Dict, Optional

from computer.computer import Computer

from .base import BaseTool, ToolError, ToolResult


class BaseEditTool(BaseTool):
    """Base class for text editor tools across different providers."""

    name = "edit"
    logger = logging.getLogger(__name__)
    computer: Computer

    def __init__(self, computer: Computer):
        """Initialize the EditTool.

        Args:
            computer: Computer instance, may be used for related operations
        """
        self.computer = computer

    async def read_file(self, path: str) -> str:
        """Read a file and return its contents.

        Args:
            path: Path to the file to read

        Returns:
            File contents as a string
        """
        try:
            path_obj = Path(path)
            if not path_obj.exists():
                raise ToolError(f"File does not exist: {path}")
            return path_obj.read_text()
        except Exception as e:
            self.logger.error(f"Error reading file: {str(e)}")
            raise ToolError(f"Failed to read file: {str(e)}")

    async def write_file(self, path: str, content: str) -> None:
        """Write content to a file.

        Args:
            path: Path to the file to write
            content: Content to write to the file
        """
        try:
            path_obj = Path(path)
            # Create parent directories if they don't exist
            path_obj.parent.mkdir(parents=True, exist_ok=True)
            path_obj.write_text(content)
        except Exception as e:
            self.logger.error(f"Error writing file: {str(e)}")
            raise ToolError(f"Failed to write file: {str(e)}")

    @abstractmethod
    async def __call__(self, **kwargs) -> ToolResult:
        """Execute the tool with the provided arguments."""
        raise NotImplementedError
