"""
Server interface for Computer API.
Provides a clean API for starting and stopping the server.
"""

import asyncio
import logging
from typing import Optional

import uvicorn
from fastapi import FastAPI

from .main import app as fastapi_app

logger = logging.getLogger(__name__)


class Server:
    """
    Server interface for Computer API.

    Usage:
        from computer_api import Server

        # Synchronous usage
        server = Server()
        server.start()  # Blocks until server is stopped

        # Asynchronous usage
        server = Server()
        await server.start_async()  # Starts server in background
        # Do other things
        await server.stop()  # Stop the server
    """

    def __init__(
        self,
        host: str = "0.0.0.0",
        port: int = 8000,
        log_level: str = "info",
        ssl_keyfile: Optional[str] = None,
        ssl_certfile: Optional[str] = None,
    ):
        """
        Initialize the server.

        Args:
            host: Host to bind the server to
            port: Port to bind the server to
            log_level: Logging level (debug, info, warning, error, critical)
            ssl_keyfile: Path to SSL private key file (for HTTPS)
            ssl_certfile: Path to SSL certificate file (for HTTPS)
        """
        self.host = host
        self.port = port
        self.log_level = log_level
        self.ssl_keyfile = ssl_keyfile
        self.ssl_certfile = ssl_certfile
        self.app = fastapi_app
        self._server_task: Optional[asyncio.Task] = None
        self._should_exit = asyncio.Event()

    def start(self) -> None:
        """
        Start the server synchronously. This will block until the server is stopped.
        """
        uvicorn.run(
            self.app,
            host=self.host,
            port=self.port,
            log_level=self.log_level,
            ssl_keyfile=self.ssl_keyfile,
            ssl_certfile=self.ssl_certfile,
        )

    async def start_async(self) -> None:
        """
        Start the server asynchronously. This will return immediately and the server
        will run in the background.
        """
        server_config = uvicorn.Config(
            self.app,
            host=self.host,
            port=self.port,
            log_level=self.log_level,
            ssl_keyfile=self.ssl_keyfile,
            ssl_certfile=self.ssl_certfile,
        )

        self._should_exit.clear()
        server = uvicorn.Server(server_config)

        # Create a task to run the server
        self._server_task = asyncio.create_task(server.serve())

        # Wait a short time to ensure the server starts
        await asyncio.sleep(0.5)

        protocol = "https" if self.ssl_certfile else "http"
        logger.info(f"Server started at {protocol}://{self.host}:{self.port}")

    async def stop(self) -> None:
        """
        Stop the server if it's running asynchronously.
        """
        if self._server_task and not self._server_task.done():
            # Signal the server to exit
            self._should_exit.set()

            # Cancel the server task
            self._server_task.cancel()

            try:
                await self._server_task
            except asyncio.CancelledError:
                logger.info("Server stopped")

            self._server_task = None
