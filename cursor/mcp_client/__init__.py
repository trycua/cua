"""
Cursor MCP Client - Connect to local MCP servers on macOS.

This package provides the client-side implementation for Cursor to connect
to and interact with MCP servers running on the local macOS environment.
"""

from .client import CursorMCPClient, ConnectionConfig
from .constants import DEFAULT_PORT, DEFAULT_HOST

__all__ = [
    "CursorMCPClient", 
    "ConnectionConfig",
    "DEFAULT_PORT", 
    "DEFAULT_HOST"
]

__version__ = "0.1.0" 