#!/usr/bin/env python3
"""Test script for Cursor MCP client functionality."""

import asyncio
import logging
import os
import sys
from typing import Optional

# Add the parent directory to the path so we can import the cursor module
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from cursor.mcp_client.main import (
    connect, 
    disconnect, 
    ensure_connected, 
    get_filesystem, 
    get_terminal
)

# Set up logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger("mcp_client_test")


async def test_filesystem():
    """Test filesystem operations."""
    logger.info("Testing filesystem operations...")
    
    fs = await get_filesystem()
    if not fs:
        logger.error("Failed to get filesystem")
        return False
    
    # List the current directory
    try:
        entries = await fs.list_directory(".")
        logger.info(f"Directory contents: {entries}")
        
        # Create a test file
        test_content = "Hello, MCP!"
        test_file = "mcp_test.txt"
        
        logger.info(f"Creating test file: {test_file}")
        await fs.save_file_content(test_file, test_content)
        
        # Read the file content
        content = await fs.get_file_content(test_file)
        logger.info(f"Read content: {content}")
        
        # Delete the test file
        logger.info(f"Deleting test file: {test_file}")
        await fs.delete_file(test_file)
        
        logger.info("Filesystem tests completed successfully")
        return True
    except Exception as e:
        logger.error(f"Filesystem test failed: {e}")
        return False


async def test_terminal():
    """Test terminal operations."""
    logger.info("Testing terminal operations...")
    
    term = await get_terminal()
    if not term:
        logger.error("Failed to get terminal")
        return False
    
    try:
        # Create a terminal
        terminal_id = await term.create_terminal()
        if not terminal_id:
            logger.error("Failed to create terminal")
            return False
            
        logger.info(f"Created terminal: {terminal_id}")
        
        # Send a command
        await term.send_input(terminal_id, "echo 'Hello from MCP terminal'\n")
        
        # Get the output
        output = await term.get_output(terminal_id)
        logger.info(f"Terminal output: {output}")
        
        # Close the terminal
        await term.close_terminal(terminal_id)
        logger.info(f"Closed terminal: {terminal_id}")
        
        logger.info("Terminal tests completed successfully")
        return True
    except Exception as e:
        logger.error(f"Terminal test failed: {e}")
        return False


async def main():
    """Main test function."""
    logger.info("Starting MCP client test")
    
    # Connect to the MCP server
    connected = await connect()
    if not connected:
        logger.error("Failed to connect to MCP server")
        return
    
    logger.info("Connected to MCP server")
    
    # Run tests
    fs_result = await test_filesystem()
    term_result = await test_terminal()
    
    # Disconnect
    await disconnect()
    logger.info("Disconnected from MCP server")
    
    # Report results
    if fs_result and term_result:
        logger.info("All tests passed!")
    else:
        logger.error("Some tests failed")


if __name__ == "__main__":
    asyncio.run(main()) 