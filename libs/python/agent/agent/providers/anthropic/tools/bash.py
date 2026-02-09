import asyncio
import os
from typing import Any, ClassVar, Dict, Literal

from computer.computer import Computer

from ....core.tools.bash import BaseBashTool
from .base import BaseAnthropicTool, CLIResult, ToolError, ToolResult


class BashTool(BaseBashTool, BaseAnthropicTool):
    """
    A tool that allows the agent to run bash commands.
    The tool parameters are defined by Anthropic and are not editable.
    """

    name: ClassVar[Literal["bash"]] = "bash"
    api_type: ClassVar[Literal["bash_20250124"]] = "bash_20250124"
    _timeout: float = 120.0  # seconds

    def __init__(self, computer: Computer):
        """Initialize the bash tool.

        Args:
            computer: Computer instance for executing commands
        """
        # Initialize the base bash tool first
        BaseBashTool.__init__(self, computer)
        # Then initialize the Anthropic tool
        BaseAnthropicTool.__init__(self)
        # Initialize bash session

    async def __call__(self, command: str | None = None, restart: bool = False, **kwargs):
        """Execute a bash command.

        Args:
            command: The command to execute
            restart: Whether to restart the shell (not used with computer interface)

        Returns:
            Tool execution result

        Raises:
            ToolError: If command execution fails
        """
        if restart:
            return ToolResult(system="Restart not needed with computer interface.")

        if command is None:
            raise ToolError("no command provided.")

        try:
            async with asyncio.timeout(self._timeout):
                result = await self.computer.interface.run_command(command)
                return CLIResult(output=result.stdout or "", error=result.stderr or "")
        except asyncio.TimeoutError as e:
            raise ToolError(f"Command timed out after {self._timeout} seconds") from e
        except Exception as e:
            raise ToolError(f"Failed to execute command: {str(e)}")

    def to_params(self) -> Dict[str, Any]:
        """Convert tool to API parameters.

        Returns:
            Dictionary with tool parameters
        """
        return {"name": self.name, "type": self.api_type}
