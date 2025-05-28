"""Main entry point for Cursor MCP client integration."""

import asyncio
import logging
import sys
from typing import Any, Callable, Dict, List, Optional, Union

from .client import ConnectionConfig, CursorMCPClient
from .constants import DEFAULT_HOST, DEFAULT_PORT
from .filesystem import FileSystem
from .terminal import Terminal
from .ui import CursorUI

logger = logging.getLogger("cursor_mcp_client.main")


class CursorMCPManager:
    """Manager for Cursor MCP client integration."""
    
    def __init__(self):
        """Initialize the MCP manager."""
        self.client: Optional[CursorMCPClient] = None
        self.filesystem: Optional[FileSystem] = None
        self.terminal: Optional[Terminal] = None
        self._connected = False
        
    async def connect(
        self, 
        host: str = DEFAULT_HOST, 
        port: int = DEFAULT_PORT,
        show_ui: bool = True,
    ) -> bool:
        """Connect to a local MCP server.
        
        Args:
            host: Server hostname or IP address.
            port: Server port.
            show_ui: Whether to show UI feedback.
            
        Returns:
            bool: True if connected, False otherwise.
        """
        if self.client and self._connected:
            return True
            
        config = ConnectionConfig(
            host=host,
            port=port,
        )
        
        self.client = CursorMCPClient(config)
        connected = await self.client.connect()
        
        if connected:
            self._connected = True
            self.filesystem = FileSystem(self.client)
            self.terminal = Terminal(self.client)
            
            if show_ui:
                tools = await self.client.list_tools()
                await CursorUI.show_connection_status(
                    connected=True,
                    server_name=f"{host}:{port}",
                    available_tools=len(tools),
                )
        else:
            if show_ui:
                await CursorUI.show_connection_status(connected=False)
                await CursorUI.show_error(
                    "Connection Failed",
                    f"Could not connect to MCP server at {host}:{port}",
                )
                
        return connected
    
    async def disconnect(self) -> None:
        """Disconnect from the MCP server."""
        if self.client:
            await self.client.disconnect()
            self.client = None
            self.filesystem = None
            self.terminal = None
            self._connected = False
    
    async def is_connected(self) -> bool:
        """Check if connected to the MCP server.
        
        Returns:
            bool: True if connected, False otherwise.
        """
        if not self.client or not self._connected:
            return False
            
        return await self.client.is_connected()
    
    async def ensure_connected(self, show_dialog: bool = True) -> bool:
        """Ensure we are connected to a MCP server.
        
        Args:
            show_dialog: Whether to show a connection dialog if not connected.
            
        Returns:
            bool: True if connected, False otherwise.
        """
        if await self.is_connected():
            return True
            
        if show_dialog:
            # Show connection dialog
            settings = await CursorUI.show_connection_dialog(
                on_connect=lambda host, port: self.connect(host, port, True)
            )
            return await self.connect(settings["host"], settings["port"])
        else:
            # Try to connect with defaults
            return await self.connect()


# Global singleton instance
manager = CursorMCPManager()


async def connect() -> bool:
    """Connect to a local MCP server.
    
    Returns:
        bool: True if connected, False otherwise.
    """
    return await manager.connect()


async def disconnect() -> None:
    """Disconnect from the MCP server."""
    await manager.disconnect()


async def is_connected() -> bool:
    """Check if connected to the MCP server.
    
    Returns:
        bool: True if connected, False otherwise.
    """
    return await manager.is_connected()


async def ensure_connected() -> bool:
    """Ensure we are connected to a MCP server.
    
    Returns:
        bool: True if connected, False otherwise.
    """
    return await manager.ensure_connected()


async def get_filesystem() -> Optional[FileSystem]:
    """Get the filesystem handler.
    
    Returns:
        Optional[FileSystem]: Filesystem handler if connected, None otherwise.
    """
    if not await ensure_connected():
        return None
        
    return manager.filesystem


async def get_terminal() -> Optional[Terminal]:
    """Get the terminal handler.
    
    Returns:
        Optional[Terminal]: Terminal handler if connected, None otherwise.
    """
    if not await ensure_connected():
        return None
        
    return manager.terminal 