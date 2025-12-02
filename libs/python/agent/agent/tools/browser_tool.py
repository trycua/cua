"""
Browser Tool for agent interactions.
Allows agents to control a browser programmatically via Playwright.
"""

import logging
from typing import Optional

import aiohttp

logger = logging.getLogger(__name__)


class BrowserTool:
    """
    Browser tool that connects to the computer server's Playwright endpoint.
    Implements the Fara/Magentic-One agent interface for browser control.
    """

    def __init__(
        self,
        base_url: str = "http://localhost:8000",
        api_key: Optional[str] = None,
        container_name: Optional[str] = None,
    ):
        """
        Initialize the BrowserTool.

        Args:
            base_url: Base URL of the computer server (default: http://localhost:8000)
            api_key: Optional API key for cloud authentication
            container_name: Optional container name for cloud authentication
        """
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.container_name = container_name
        self.logger = logger

    def _get_endpoint_url(self) -> str:
        """Get the full URL for the playwright_exec endpoint."""
        return f"{self.base_url}/playwright_exec"

    def _get_headers(self) -> dict:
        """Get headers for the HTTP request."""
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["X-API-Key"] = self.api_key
        if self.container_name:
            headers["X-Container-Name"] = self.container_name
        return headers

    async def _execute_command(self, command: str, params: dict) -> dict:
        """
        Execute a browser command via HTTP POST.

        Args:
            command: Command name
            params: Command parameters

        Returns:
            Response dictionary
        """
        url = self._get_endpoint_url()
        payload = {"command": command, "params": params}
        headers = self._get_headers()

        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(url, json=payload, headers=headers) as response:
                    if response.status == 200:
                        return await response.json()
                    else:
                        error_text = await response.text()
                        self.logger.error(
                            f"Browser command failed with status {response.status}: {error_text}"
                        )
                        return {"success": False, "error": error_text}
        except Exception as e:
            self.logger.error(f"Error executing browser command: {e}")
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

