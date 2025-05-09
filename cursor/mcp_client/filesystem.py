"""File system operations through MCP."""

import logging
import os
import pathlib
from typing import Any, Dict, List, Optional, Union

from .client import CursorMCPClient

logger = logging.getLogger("cursor_mcp_client.filesystem")


class FileSystem:
    """File system operations through MCP."""
    
    def __init__(self, client: CursorMCPClient):
        """Initialize the file system handler.
        
        Args:
            client: The MCP client instance.
        """
        self.client = client
        
    async def list_directory(self, path: str) -> List[Dict[str, Any]]:
        """List contents of a directory.
        
        Args:
            path: Path to the directory.
            
        Returns:
            List[Dict[str, Any]]: List of directory entries (files/folders).
        """
        try:
            result = await self.client.call_tool("list_directory", {"path": path})
            return result.get("entries", [])
        except Exception as e:
            logger.error(f"Error listing directory {path}: {str(e)}")
            return []
    
    async def get_file_content(self, path: str) -> Optional[str]:
        """Get the content of a file.
        
        Args:
            path: Path to the file.
            
        Returns:
            Optional[str]: File content, or None if an error occurred.
        """
        try:
            result = await self.client.call_tool("read_file", {"path": path})
            return result.get("content")
        except Exception as e:
            logger.error(f"Error reading file {path}: {str(e)}")
            return None
    
    async def save_file_content(self, path: str, content: str) -> bool:
        """Save content to a file.
        
        Args:
            path: Path to the file.
            content: Content to write.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        try:
            result = await self.client.call_tool("write_file", {
                "path": path,
                "content": content
            })
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Error writing to file {path}: {str(e)}")
            return False
    
    async def create_directory(self, path: str) -> bool:
        """Create a directory.
        
        Args:
            path: Path to the directory.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        try:
            result = await self.client.call_tool("create_directory", {"path": path})
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Error creating directory {path}: {str(e)}")
            return False
    
    async def delete_file(self, path: str) -> bool:
        """Delete a file.
        
        Args:
            path: Path to the file.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        try:
            result = await self.client.call_tool("delete_file", {"path": path})
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Error deleting file {path}: {str(e)}")
            return False
    
    async def delete_directory(self, path: str, recursive: bool = False) -> bool:
        """Delete a directory.
        
        Args:
            path: Path to the directory.
            recursive: Whether to delete recursively.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        try:
            result = await self.client.call_tool("delete_directory", {
                "path": path,
                "recursive": recursive
            })
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Error deleting directory {path}: {str(e)}")
            return False
    
    async def file_exists(self, path: str) -> bool:
        """Check if a file exists.
        
        Args:
            path: Path to check.
            
        Returns:
            bool: True if the file exists, False otherwise.
        """
        try:
            result = await self.client.call_tool("file_exists", {"path": path})
            return result.get("exists", False)
        except Exception as e:
            logger.error(f"Error checking if file exists {path}: {str(e)}")
            return False
    
    async def get_file_info(self, path: str) -> Optional[Dict[str, Any]]:
        """Get information about a file.
        
        Args:
            path: Path to the file.
            
        Returns:
            Optional[Dict[str, Any]]: File information, or None if an error occurred.
        """
        try:
            result = await self.client.call_tool("file_info", {"path": path})
            return result
        except Exception as e:
            logger.error(f"Error getting file info for {path}: {str(e)}")
            return None 