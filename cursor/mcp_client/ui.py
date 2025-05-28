"""UI components for Cursor MCP client integration."""

import asyncio
import sys
from typing import Any, Callable, Dict, Optional


class CursorUI:
    """Cursor UI integration for the MCP client."""
    
    @staticmethod
    async def show_connection_dialog(
        title: str = "Connect to Local MCP Server",
        host_default: str = "127.0.0.1",
        port_default: int = 7681,
        on_connect: Optional[Callable[[str, int], Any]] = None
    ) -> Dict[str, Any]:
        """Show a connection dialog in Cursor.
        
        Args:
            title: Dialog title.
            host_default: Default host value.
            port_default: Default port value.
            on_connect: Callback function called when connection is established.
            
        Returns:
            Dict[str, Any]: User's connection settings.
        """
        # This is a placeholder for actual Cursor UI API integration
        # In a real implementation, this would show a dialog in the Cursor UI
        
        # For now, we'll use a command-line implementation for testing
        print(f"\n{title}")
        print("=" * len(title))
        
        host = input(f"Host [{host_default}]: ") or host_default
        
        port_str = input(f"Port [{port_default}]: ") or str(port_default)
        try:
            port = int(port_str)
        except ValueError:
            print(f"Invalid port number, using default: {port_default}")
            port = port_default
            
        result = {"host": host, "port": port}
        
        if on_connect:
            await on_connect(host, port)
            
        return result
    
    @staticmethod
    async def show_connection_status(
        connected: bool, 
        server_name: Optional[str] = None,
        available_tools: Optional[int] = None
    ) -> None:
        """Show connection status in the Cursor UI.
        
        Args:
            connected: Whether the connection is active.
            server_name: Name of the connected server.
            available_tools: Number of available tools.
        """
        # Placeholder for actual Cursor UI integration
        if connected:
            status = f"Connected to MCP server: {server_name or 'Unknown'}"
            if available_tools is not None:
                status += f" ({available_tools} tools available)"
        else:
            status = "Not connected to MCP server"
            
        print(status)
    
    @staticmethod
    async def show_error(title: str, message: str) -> None:
        """Show an error message in the Cursor UI.
        
        Args:
            title: Error title.
            message: Error message.
        """
        # Placeholder for actual Cursor UI integration
        print(f"\nError: {title}")
        print(message)
    
    @staticmethod
    async def show_success(title: str, message: str) -> None:
        """Show a success message in the Cursor UI.
        
        Args:
            title: Success title.
            message: Success message.
        """
        # Placeholder for actual Cursor UI integration
        print(f"\nSuccess: {title}")
        print(message) 