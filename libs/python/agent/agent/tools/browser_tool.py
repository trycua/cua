"""
Browser Tool for agent interactions.
Allows agents to control a browser programmatically via Playwright.
"""

import logging
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from computer.interface import GenericComputerInterface

logger = logging.getLogger(__name__)


class BrowserTool:
    """
    Browser tool that uses the computer SDK's interface to control a browser.
    Implements the Fara/Magentic-One agent interface for browser control.
    """

    def __init__(
        self,
        interface: "GenericComputerInterface",
    ):
        """
        Initialize the BrowserTool.

        Args:
            interface: A GenericComputerInterface instance that provides playwright_exec
        """
        self.interface = interface
        self.logger = logger

    async def _execute_command(self, command: str, params: dict) -> dict:
        """
        Execute a browser command via the computer interface.

        Args:
            command: Command name
            params: Command parameters

        Returns:
            Response dictionary
        """
        try:
            result = await self.interface.playwright_exec(command, params)
            if not result.get("success"):
                self.logger.error(
                    f"Browser command '{command}' failed: {result.get('error', 'Unknown error')}"
                )
            return result
        except Exception as e:
            self.logger.error(f"Error executing browser command '{command}': {e}")
            return {"success": False, "error": str(e)}

    async def visit_url(self, url: str) -> dict:
        """
        Navigate to a URL.

        Args:
            url: URL to visit

        Returns:
            Response dictionary with success status and current URL
        """
        return await self._execute_command("visit_url", {"url": url})

    async def click(self, x: int, y: int) -> dict:
        """
        Click at coordinates.

        Args:
            x: X coordinate
            y: Y coordinate

        Returns:
            Response dictionary with success status
        """
        return await self._execute_command("click", {"x": x, "y": y})

    async def type(self, text: str) -> dict:
        """
        Type text into the focused element.

        Args:
            text: Text to type

        Returns:
            Response dictionary with success status
        """
        return await self._execute_command("type", {"text": text})

    async def scroll(self, delta_x: int, delta_y: int) -> dict:
        """
        Scroll the page.

        Args:
            delta_x: Horizontal scroll delta
            delta_y: Vertical scroll delta

        Returns:
            Response dictionary with success status
        """
        return await self._execute_command("scroll", {"delta_x": delta_x, "delta_y": delta_y})

    async def web_search(self, query: str) -> dict:
        """
        Navigate to a Google search for the query.

        Args:
            query: Search query

        Returns:
            Response dictionary with success status and current URL
        """
        return await self._execute_command("web_search", {"query": query})
