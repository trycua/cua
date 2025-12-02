"""
Browser Tool Example

Demonstrates how to use the BrowserTool to control a browser programmatically
via the computer server. The browser runs visibly on the XFCE desktop so visual
agents can see it.

Prerequisites:
    - Computer server running (Docker container or local)
    - For Docker: Container should be running with browser tool support
    - For local: Playwright and Firefox must be installed

Usage:
    python examples/browser_tool_example.py
"""

import asyncio
import logging
import sys
from pathlib import Path

# Import BrowserTool directly from the file
browser_tool_path = Path(__file__).parent.parent / "libs" / "python" / "agent" / "agent" / "tools" / "browser_tool.py"
sys.path.insert(0, str(browser_tool_path.parent.parent.parent))

# Import the module directly
import importlib.util
spec = importlib.util.spec_from_file_location("browser_tool", browser_tool_path)
if spec is None or spec.loader is None:
    raise ImportError(f"Could not load browser_tool from {browser_tool_path}")
browser_tool_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(browser_tool_module)
BrowserTool = browser_tool_module.BrowserTool

# Configure logging to see what's happening
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def test_browser_tool():
    """Test the BrowserTool with various commands."""
    
    # Initialize the browser tool
    # For local testing, use http://localhost:8000
    # For cloud, provide base_url, api_key, and container_name
    browser = BrowserTool(base_url="http://localhost:8000")
    
    logger.info("Testing Browser Tool...")
    
    try:
        # Test 1: Visit a URL
        logger.info("Test 1: Visiting a URL...")
        result = await browser.visit_url("https://www.trycua.com")
        logger.info(f"Visit URL result: {result}")
        
        # Wait a bit for the page to load
        await asyncio.sleep(2)
        
        # Test 2: Web search
        logger.info("Test 2: Performing a web search...")
        result = await browser.web_search("Python programming")
        logger.info(f"Web search result: {result}")
        
        # Wait a bit
        await asyncio.sleep(2)
        
        # Test 3: Scroll
        logger.info("Test 3: Scrolling the page...")
        result = await browser.scroll(delta_x=0, delta_y=500)
        logger.info(f"Scroll result: {result}")
        
        # Wait a bit
        await asyncio.sleep(1)
        
        # Test 4: Click (example coordinates - adjust based on your screen)
        logger.info("Test 4: Clicking at coordinates...")
        result = await browser.click(x=500, y=300)
        logger.info(f"Click result: {result}")
        
        # Wait a bit
        await asyncio.sleep(1)
        
        # Test 5: Type text (if there's a focused input field)
        logger.info("Test 5: Typing text...")
        result = await browser.type("Hello from BrowserTool!")
        logger.info(f"Type result: {result}")
        
        logger.info("All tests completed!")
        
    except Exception as e:
        logger.error(f"Error during testing: {e}", exc_info=True)


if __name__ == "__main__":
    asyncio.run(test_browser_tool())

