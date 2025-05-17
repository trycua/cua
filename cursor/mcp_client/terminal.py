"""Terminal operations through MCP."""

import asyncio
import logging
from typing import Any, Callable, Dict, List, Optional, Union

from .client import CursorMCPClient

logger = logging.getLogger("cursor_mcp_client.terminal")


class Terminal:
    """Terminal operations through MCP."""
    
    def __init__(self, client: CursorMCPClient):
        """Initialize the terminal handler.
        
        Args:
            client: The MCP client instance.
        """
        self.client = client
        self.terminals: Dict[str, Dict[str, Any]] = {}
        
    async def create_terminal(
        self, 
        working_directory: Optional[str] = None,
        env_vars: Optional[Dict[str, str]] = None
    ) -> Optional[str]:
        """Create a new terminal session.
        
        Args:
            working_directory: Initial working directory.
            env_vars: Environment variables.
            
        Returns:
            Optional[str]: Terminal ID if successful, None otherwise.
        """
        try:
            args: Dict[str, Any] = {}
            
            if working_directory:
                args["working_directory"] = working_directory
                
            if env_vars:
                args["environment"] = env_vars
                
            result = await self.client.call_tool("create_terminal", args)
            
            if not result or "terminal_id" not in result:
                logger.error("Failed to create terminal session")
                return None
                
            terminal_id = result["terminal_id"]
            self.terminals[terminal_id] = {
                "id": terminal_id,
                "working_directory": working_directory,
                "created_at": result.get("created_at"),
            }
            
            return terminal_id
        except Exception as e:
            logger.error(f"Error creating terminal session: {str(e)}")
            return None
    
    async def send_input(self, terminal_id: str, input_text: str) -> bool:
        """Send input to a terminal session.
        
        Args:
            terminal_id: Terminal ID.
            input_text: Text to send.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        if terminal_id not in self.terminals:
            logger.error(f"Terminal {terminal_id} not found")
            return False
            
        try:
            result = await self.client.call_tool("terminal_input", {
                "terminal_id": terminal_id,
                "input": input_text
            })
            
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Error sending input to terminal {terminal_id}: {str(e)}")
            return False
    
    async def get_output(self, terminal_id: str, since: Optional[int] = None) -> Optional[Dict[str, Any]]:
        """Get output from a terminal session.
        
        Args:
            terminal_id: Terminal ID.
            since: Sequence number to get output since.
            
        Returns:
            Optional[Dict[str, Any]]: Terminal output.
        """
        if terminal_id not in self.terminals:
            logger.error(f"Terminal {terminal_id} not found")
            return None
            
        try:
            args: Dict[str, Any] = {"terminal_id": terminal_id}
            
            if since is not None:
                args["since"] = since
                
            return await self.client.call_tool("terminal_output", args)
        except Exception as e:
            logger.error(f"Error getting output from terminal {terminal_id}: {str(e)}")
            return None
    
    async def resize_terminal(self, terminal_id: str, cols: int, rows: int) -> bool:
        """Resize a terminal session.
        
        Args:
            terminal_id: Terminal ID.
            cols: Number of columns.
            rows: Number of rows.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        if terminal_id not in self.terminals:
            logger.error(f"Terminal {terminal_id} not found")
            return False
            
        try:
            result = await self.client.call_tool("resize_terminal", {
                "terminal_id": terminal_id,
                "cols": cols,
                "rows": rows
            })
            
            return result.get("success", False)
        except Exception as e:
            logger.error(f"Error resizing terminal {terminal_id}: {str(e)}")
            return False
    
    async def close_terminal(self, terminal_id: str) -> bool:
        """Close a terminal session.
        
        Args:
            terminal_id: Terminal ID.
            
        Returns:
            bool: True if successful, False otherwise.
        """
        if terminal_id not in self.terminals:
            logger.error(f"Terminal {terminal_id} not found")
            return False
            
        try:
            result = await self.client.call_tool("close_terminal", {
                "terminal_id": terminal_id
            })
            
            success = result.get("success", False)
            
            if success:
                self.terminals.pop(terminal_id, None)
                
            return success
        except Exception as e:
            logger.error(f"Error closing terminal {terminal_id}: {str(e)}")
            return False
    
    async def list_terminals(self) -> List[Dict[str, Any]]:
        """List all active terminals.
        
        Returns:
            List[Dict[str, Any]]: List of terminal information.
        """
        try:
            result = await self.client.call_tool("list_terminals", {})
            
            if not result or "terminals" not in result:
                return []
                
            # Update our local cache
            for terminal in result["terminals"]:
                terminal_id = terminal.get("id")
                if terminal_id:
                    self.terminals[terminal_id] = terminal
                    
            return result["terminals"]
        except Exception as e:
            logger.error(f"Error listing terminals: {str(e)}")
            return []
    
    async def attach_output_handler(
        self,
        terminal_id: str,
        handler: Callable[[str], Any],
        interval: float = 0.5
    ) -> asyncio.Task:
        """Attach a handler for terminal output.
        
        Args:
            terminal_id: Terminal ID.
            handler: Function to call with output.
            interval: Polling interval in seconds.
            
        Returns:
            asyncio.Task: The polling task.
        """
        async def _poll_output():
            last_seq = None
            
            while True:
                try:
                    output = await self.get_output(terminal_id, last_seq)
                    
                    if output and "output" in output:
                        handler(output["output"])
                        last_seq = output.get("sequence")
                except asyncio.CancelledError:
                    break
                except Exception as e:
                    logger.error(f"Error in terminal output polling: {str(e)}")
                    
                await asyncio.sleep(interval)
                
        return asyncio.create_task(_poll_output()) 