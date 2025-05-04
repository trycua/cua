"""
MCP Client implementation for Cursor to connect to local macOS MCP servers.
"""

import asyncio
import json
import logging
import os
import socket
import sys
import time
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple, Union

from .constants import (
    DEFAULT_CONNECTION_TIMEOUT,
    DEFAULT_HOST,
    DEFAULT_PORT,
    LOG_FORMAT,
    MAX_RETRIES,
    RETRY_DELAY,
)

# Configure logging
logging.basicConfig(level=logging.INFO, format=LOG_FORMAT)
logger = logging.getLogger("cursor_mcp_client")


@dataclass
class ConnectionConfig:
    """Configuration for MCP server connection."""

    host: str = DEFAULT_HOST
    port: int = DEFAULT_PORT
    timeout: float = DEFAULT_CONNECTION_TIMEOUT


class CursorMCPClient:
    """Client for connecting to a local MCP server from Cursor."""

    def __init__(self, config: Optional[ConnectionConfig] = None):
        """Initialize the MCP client.
        
        Args:
            config: Connection configuration. If None, default values are used.
        """
        self.config = config or ConnectionConfig()
        self.reader: Optional[asyncio.StreamReader] = None
        self.writer: Optional[asyncio.StreamWriter] = None
        self.message_id = 0
        self._connected = False

    async def connect(self) -> bool:
        """Connect to the MCP server.
        
        Returns:
            bool: True if the connection was successful, False otherwise.
        """
        for attempt in range(MAX_RETRIES):
            try:
                logger.info(f"Attempting to connect to MCP server at {self.config.host}:{self.config.port} (attempt {attempt+1}/{MAX_RETRIES})")
                self.reader, self.writer = await asyncio.wait_for(
                    asyncio.open_connection(self.config.host, self.config.port),
                    timeout=self.config.timeout,
                )
                logger.info(f"Successfully connected to MCP server at {self.config.host}:{self.config.port}")
                
                # Initialize the connection
                success = await self._initialize_connection()
                if success:
                    self._connected = True
                    return True
                else:
                    logger.error("Failed to initialize MCP connection")
                    await self.disconnect()
            except (ConnectionRefusedError, socket.gaierror) as e:
                logger.warning(f"Connection attempt {attempt+1} failed: {str(e)}")
                if attempt < MAX_RETRIES - 1:
                    logger.info(f"Retrying in {RETRY_DELAY} seconds...")
                    await asyncio.sleep(RETRY_DELAY)
                else:
                    logger.error(f"Failed to connect after {MAX_RETRIES} attempts")
            except asyncio.TimeoutError:
                logger.warning(f"Connection attempt {attempt+1} timed out")
                if attempt < MAX_RETRIES - 1:
                    logger.info(f"Retrying in {RETRY_DELAY} seconds...")
                    await asyncio.sleep(RETRY_DELAY)
                else:
                    logger.error(f"Connection timed out after {MAX_RETRIES} attempts")
            except Exception as e:
                logger.error(f"Unexpected error during connection: {str(e)}")
                break
        
        return False

    async def disconnect(self) -> None:
        """Disconnect from the MCP server."""
        if self.writer:
            try:
                self.writer.close()
                await self.writer.wait_closed()
            except Exception as e:
                logger.warning(f"Error during disconnect: {str(e)}")
        
        self.reader = None
        self.writer = None
        self._connected = False
        logger.info("Disconnected from MCP server")

    async def is_connected(self) -> bool:
        """Check if the client is connected to the MCP server.
        
        Returns:
            bool: True if connected, False otherwise.
        """
        if not self._connected or not self.writer or self.writer.is_closing():
            return False
        
        try:
            # Simple ping message to check connection
            response = await self._send_message({
                "type": "ping"
            })
            return response is not None and response.get("type") == "pong"
        except Exception:
            self._connected = False
            return False

    async def list_tools(self) -> List[Dict[str, Any]]:
        """Retrieve the list of available tools from the MCP server.
        
        Returns:
            List[Dict[str, Any]]: List of tool definitions.
        """
        if not self._connected:
            if not await self.connect():
                logger.error("Failed to connect to MCP server")
                return []
        
        response = await self._send_message({
            "type": "list_tools"
        })
        
        if not response or "tools" not in response:
            logger.error("Failed to retrieve tools from MCP server")
            return []
        
        return response["tools"]

    async def call_tool(self, tool_name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        """Call a tool on the MCP server.
        
        Args:
            tool_name: Name of the tool to call.
            arguments: Arguments to pass to the tool.
            
        Returns:
            Dict[str, Any]: Tool execution result.
        """
        if not self._connected:
            if not await self.connect():
                raise ConnectionError("Failed to connect to MCP server")
        
        response = await self._send_message({
            "type": "call_tool",
            "name": tool_name,
            "arguments": arguments
        })
        
        if not response:
            raise Exception(f"Failed to call tool {tool_name}")
        
        return response

    async def read_resource(self, uri: str, parameters: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        """Read a resource from the MCP server.
        
        Args:
            uri: The resource URI.
            parameters: Optional parameters for parameterized resources.
            
        Returns:
            Dict[str, Any]: Resource content.
        """
        if not self._connected:
            if not await self.connect():
                raise ConnectionError("Failed to connect to MCP server")
        
        message: Dict[str, Any] = {
            "type": "read_resource",
            "uri": uri
        }
        
        if parameters:
            message["parameters"] = parameters
        
        response = await self._send_message(message)
        
        if not response:
            raise Exception(f"Failed to read resource {uri}")
        
        return response

    async def get_file_system_tree(self, root_path: str, depth: int = 2) -> Dict[str, Any]:
        """Get the file system tree from the MCP server.
        
        Args:
            root_path: The root path to start from.
            depth: How many levels to explore (default: 2).
            
        Returns:
            Dict[str, Any]: File system tree.
        """
        try:
            # This likely uses a specific tool or resource on the MCP server
            return await self.call_tool("get_file_system", {
                "path": root_path,
                "depth": depth
            })
        except Exception as e:
            logger.error(f"Failed to get file system tree: {str(e)}")
            return {"error": str(e)}

    async def open_file(self, file_path: str) -> Dict[str, Any]:
        """Open a file through the MCP server.
        
        Args:
            file_path: Path to the file on the local Mac.
            
        Returns:
            Dict[str, Any]: File content.
        """
        try:
            return await self.call_tool("open_file", {
                "path": file_path
            })
        except Exception as e:
            logger.error(f"Failed to open file {file_path}: {str(e)}")
            return {"error": str(e)}

    async def save_file(self, file_path: str, content: str) -> Dict[str, Any]:
        """Save a file through the MCP server.
        
        Args:
            file_path: Path to the file on the local Mac.
            content: File content to write.
            
        Returns:
            Dict[str, Any]: Operation result.
        """
        try:
            return await self.call_tool("save_file", {
                "path": file_path,
                "content": content
            })
        except Exception as e:
            logger.error(f"Failed to save file {file_path}: {str(e)}")
            return {"error": str(e)}

    async def create_terminal(self, working_dir: Optional[str] = None) -> Dict[str, Any]:
        """Create a new terminal session on the MCP server.
        
        Args:
            working_dir: Optional working directory for the terminal.
            
        Returns:
            Dict[str, Any]: Terminal session info.
        """
        try:
            args = {}
            if working_dir:
                args["working_directory"] = working_dir
                
            return await self.call_tool("create_terminal", args)
        except Exception as e:
            logger.error(f"Failed to create terminal: {str(e)}")
            return {"error": str(e)}

    async def send_terminal_input(self, terminal_id: str, input_text: str) -> Dict[str, Any]:
        """Send input to a terminal session.
        
        Args:
            terminal_id: ID of the terminal session.
            input_text: Text to send to the terminal.
            
        Returns:
            Dict[str, Any]: Operation result.
        """
        try:
            return await self.call_tool("terminal_input", {
                "terminal_id": terminal_id,
                "input": input_text
            })
        except Exception as e:
            logger.error(f"Failed to send terminal input: {str(e)}")
            return {"error": str(e)}

    async def _next_message_id(self) -> int:
        """Generate a unique message ID.
        
        Returns:
            int: New message ID.
        """
        self.message_id += 1
        return self.message_id

    async def _initialize_connection(self) -> bool:
        """Initialize the MCP connection.
        
        Returns:
            bool: True if initialization was successful.
        """
        try:
            message = {
                "type": "initialize",
                "id": await self._next_message_id(),
                "implementation": {
                    "name": "cursor-mcp-client",
                    "version": "0.1.0"
                }
            }
            
            if not self.writer or not self.reader:
                return False
                
            await self._send_raw(message)
            response = await self._receive_raw()
            
            if not response or response.get("type") != "initialize_response":
                logger.error(f"Invalid initialization response: {response}")
                return False
                
            return True
        except Exception as e:
            logger.error(f"Error during MCP initialization: {str(e)}")
            return False

    async def _send_message(self, message: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Send a message to the MCP server and wait for a response.
        
        Args:
            message: The message to send.
            
        Returns:
            Optional[Dict[str, Any]]: The server's response, or None if an error occurred.
        """
        if not self._connected:
            await self.connect()

        if not self.writer or not self.reader:
            logger.error("No connection to MCP server")
            return None
            
        try:
            # Add message ID
            complete_message = message.copy()
            complete_message["id"] = await self._next_message_id()
            
            await self._send_raw(complete_message)
            response = await self._receive_raw()
            
            return response
        except Exception as e:
            logger.error(f"Error in MCP communication: {str(e)}")
            self._connected = False
            return None

    async def _send_raw(self, message: Dict[str, Any]) -> None:
        """Send a raw message to the MCP server.
        
        Args:
            message: The message to send.
        """
        if not self.writer:
            raise ConnectionError("Not connected to MCP server")
            
        try:
            data = json.dumps(message).encode("utf-8")
            self.writer.write(data + b"\n")
            await self.writer.drain()
        except Exception as e:
            logger.error(f"Error sending message: {str(e)}")
            self._connected = False
            raise

    async def _receive_raw(self) -> Optional[Dict[str, Any]]:
        """Receive a raw message from the MCP server.
        
        Returns:
            Optional[Dict[str, Any]]: The received message, or None if an error occurred.
        """
        if not self.reader:
            raise ConnectionError("Not connected to MCP server")
            
        try:
            data = await self.reader.readline()
            if not data:
                logger.error("Empty response from MCP server")
                self._connected = False
                return None
                
            response = json.loads(data.decode("utf-8"))
            return response
        except json.JSONDecodeError as e:
            logger.error(f"Error decoding JSON response: {str(e)}")
            return None
        except Exception as e:
            logger.error(f"Error receiving message: {str(e)}")
            self._connected = False
            raise

    async def __aenter__(self):
        """Async context manager entry."""
        await self.connect()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        await self.disconnect() 