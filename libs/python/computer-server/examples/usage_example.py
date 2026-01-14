#!/usr/bin/env python3
"""
Example showing how to use the Cua Computer API as an imported package.
"""

import asyncio
import logging
from typing import TYPE_CHECKING

# For type checking only
if TYPE_CHECKING:
    from computer_api import Server

# Setup logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)
logger = logging.getLogger(__name__)


# Example 1: Synchronous usage (blocks until server is stopped)
def example_sync():
    """
    Example of synchronous server usage. This will block until interrupted.
    Run with: python3 -m examples.usage_example sync
    """
    # Import directly to avoid any confusion
    from computer_api.server import Server

    server = Server(port=8080)
    print("Server started at http://localhost:8080")
    print("Press Ctrl+C to stop the server")

    try:
        server.start()  # This will block until the server is stopped
    except KeyboardInterrupt:
        print("Server stopped by user")


# Example 2: Asynchronous usage
async def example_async():
    """
    Example of asynchronous server usage. This will start the server in the background
    and allow other operations to run concurrently.
    Run with: python3 -m examples.usage_example async
    """
    # Import directly to avoid any confusion
    from computer_api.server import Server

    server = Server(port=8080)

    # Start the server in the background
    await server.start_async()

    print("Server is running in the background")
    print("Performing other tasks...")

    # Do other things while the server is running
    for i in range(5):
        print(f"Doing work iteration {i+1}/5...")
        await asyncio.sleep(2)

    print("Work complete, stopping server...")

    # Stop the server when done
    await server.stop()
    print("Server stopped")


if __name__ == "__main__":
    import sys

    if len(sys.argv) > 1 and sys.argv[1] == "async":
        asyncio.run(example_async())
    else:
        example_sync()
