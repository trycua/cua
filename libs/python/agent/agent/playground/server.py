"""Playground server implementation for Cua agents."""

import asyncio
import logging
import os
import platform
import socket
import traceback
import webbrowser
from typing import Any, Dict, List, Optional, Union
from urllib.parse import quote

import uvicorn
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

logger = logging.getLogger(__name__)


class PlaygroundServer:
    """Playground server for running Cua agents via HTTP API."""

    def __init__(self, agent_instance=None):
        """
        Initialize the playground server.

        Args:
            agent_instance: Optional pre-configured agent instance to use
        """
        self.agent_instance = agent_instance
        self.app = FastAPI(
            title="Cua Playground Server",
            description="Playground server for Cua agents",
            version="0.1.0",
        )
        self._setup_middleware()
        self._setup_routes()
        self.server = None
        self.port = None

    def _setup_middleware(self):
        """Setup CORS middleware."""
        self.app.add_middleware(
            CORSMiddleware,
            allow_origins=["*"],
            allow_credentials=True,
            allow_methods=["*"],
            allow_headers=["*"],
        )

    def _setup_routes(self):
        """Setup API routes."""

        @self.app.get("/status")
        async def status():
            """Health check endpoint."""
            sys = platform.system().lower()
            if "darwin" in sys or sys in ("macos", "mac"):
                os_type = "macos"
            elif "windows" in sys:
                os_type = "windows"
            else:
                os_type = "linux"

            return {
                "status": "ok",
                "os_type": os_type,
                "features": ["agent", "playground"],
            }

        @self.app.post("/responses")
        async def responses_endpoint(request: Request):
            """
            Run ComputerAgent for up to 2 turns.

            Body JSON:
            {
              "model": "...",                 # required
              "input": "... or messages[]",   # required
              "agent_kwargs": { ... },         # optional, passed directly to ComputerAgent
              "env": { ... }                   # optional env overrides for agent
            }
            """
            # Import here to avoid circular imports
            try:
                from agent import ComputerAgent
            except ImportError:
                raise HTTPException(status_code=501, detail="ComputerAgent not available")

            # Parse request body
            try:
                body = await request.json()
            except Exception as e:
                raise HTTPException(status_code=400, detail=f"Invalid JSON body: {str(e)}")

            model = body.get("model")
            input_data = body.get("input")
            if not model or input_data is None:
                raise HTTPException(status_code=400, detail="'model' and 'input' are required")

            agent_kwargs: Dict[str, Any] = body.get("agent_kwargs") or {}
            env_overrides: Dict[str, str] = body.get("env") or {}

            # Simple env override context
            class _EnvOverride:
                def __init__(self, overrides: Dict[str, str]):
                    self.overrides = overrides
                    self._original: Dict[str, Optional[str]] = {}

                def __enter__(self):
                    for k, v in (self.overrides or {}).items():
                        self._original[k] = os.environ.get(k)
                        os.environ[k] = str(v)

                def __exit__(self, exc_type, exc, tb):
                    for k, old in self._original.items():
                        if old is None:
                            os.environ.pop(k, None)
                        else:
                            os.environ[k] = old

            # Convert input to messages
            def _to_messages(data: Union[str, List[Dict[str, Any]]]) -> List[Dict[str, Any]]:
                if isinstance(data, str):
                    return [{"role": "user", "content": data}]
                if isinstance(data, list):
                    return data
                return []

            messages = _to_messages(input_data)

            error = None

            with _EnvOverride(env_overrides):
                # Use pre-configured agent if available, otherwise create new one
                if self.agent_instance:
                    agent = self.agent_instance
                else:
                    agent = ComputerAgent(model=model, **agent_kwargs)  # type: ignore[arg-type]

                total_output: List[Any] = []
                total_usage: Dict[str, Any] = {}

                pending_computer_call_ids = set()
                try:
                    async for result in agent.run(messages):
                        total_output += result["output"]
                        # Try to collect usage if present
                        if (
                            isinstance(result, dict)
                            and "usage" in result
                            and isinstance(result["usage"], dict)
                        ):
                            # Merge usage counters
                            for k, v in result["usage"].items():
                                if isinstance(v, (int, float)):
                                    total_usage[k] = total_usage.get(k, 0) + v
                                else:
                                    total_usage[k] = v
                        for msg in result.get("output", []):
                            if msg.get("type") == "computer_call":
                                pending_computer_call_ids.add(msg["call_id"])
                            elif msg.get("type") == "computer_call_output":
                                pending_computer_call_ids.discard(msg["call_id"])
                            elif msg.get("type") == "function_call":
                                pending_computer_call_ids.add(msg["call_id"])
                            elif msg.get("type") == "function_call_output":
                                pending_computer_call_ids.discard(msg["call_id"])
                        # exit if no pending computer calls
                        if not pending_computer_call_ids:
                            break
                except Exception as e:
                    logger.error(f"Error running agent: {str(e)}")
                    logger.error(traceback.format_exc())
                    error = str(e)

            # Build response payload
            payload = {
                "model": model,
                "error": error,
                "output": total_output,
                "usage": total_usage,
                "status": "completed" if not error else "failed",
            }

            # CORS: allow any origin
            headers = {
                "Cache-Control": "no-cache",
                "Connection": "keep-alive",
            }

            return JSONResponse(content=payload, headers=headers)

    def _find_available_port(self, start_port: int = 8000, max_attempts: int = 100) -> int:
        """Find an available port starting from start_port."""
        for port in range(start_port, start_port + max_attempts):
            try:
                with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                    s.bind(("127.0.0.1", port))
                    return port
            except OSError:
                continue
        raise RuntimeError(
            f"Could not find an available port in range {start_port}-{start_port + max_attempts}"
        )

    async def start_async(self, port: Optional[int] = None, open_browser: bool = False):
        """
        Start the playground server asynchronously.

        Args:
            port: Port to run the server on. If None, finds an available port.
            open_browser: Whether to open the browser automatically.
        """
        if port is None:
            port = self._find_available_port()

        self.port = port
        host = f"http://localhost:{port}"

        logger.info(f"Starting playground server on {host}")

        if open_browser:
            # Construct the playground URL
            encoded_host = quote(host, safe="")
            encoded_model = quote(self.agent_instance.model, safe="")
            encoded_vnc_url = quote("http://localhost:8006/?autoconnect=true", safe="")

            # Build URL with custom_model if agent instance is configured
            playground_url = (
                # f"http://cua.ai/dashboard/playground"
                f"http://localhost:3000/dashboard/playground"
                f"?host={encoded_host}"
                f"&port={port}"
                f"&id=localhost"
                f"&name=localhost"
                f"&custom_model={encoded_model}"
                f"&custom_vnc_url={encoded_vnc_url}"
                f"&vnc_password=null"
                f"&resize=scale"
                f"&fullscreen=true"
            )

            logger.info(f"Opening browser at: {playground_url}")
            webbrowser.open(playground_url)

        config = uvicorn.Config(
            self.app,
            host="0.0.0.0",
            port=port,
            log_level="info",
        )
        self.server = uvicorn.Server(config)
        await self.server.serve()

    def start(self, port: Optional[int] = None, open_browser: bool = False):
        """
        Start the playground server (blocking).

        Args:
            port: Port to run the server on. If None, finds an available port.
            open_browser: Whether to open the browser automatically.
        """
        # Check if there's already a running event loop
        try:
            loop = asyncio.get_running_loop()
            # If we're in an async context, schedule as a task
            import threading

            # Run the server in a separate thread to avoid blocking
            server_thread = threading.Thread(
                target=self._run_in_new_loop,
                args=(port, open_browser),
                daemon=True,
            )
            server_thread.start()

            # Give the server a moment to start and open browser
            import time

            time.sleep(1)

        except RuntimeError:
            # No running loop, can use asyncio.run() safely
            asyncio.run(self.start_async(port=port, open_browser=open_browser))

    def _run_in_new_loop(self, port: Optional[int] = None, open_browser: bool = False):
        """Helper to run server in a new event loop (for threading)."""
        new_loop = asyncio.new_event_loop()
        asyncio.set_event_loop(new_loop)
        try:
            new_loop.run_until_complete(self.start_async(port=port, open_browser=open_browser))
        finally:
            new_loop.close()

    async def stop(self):
        """Stop the playground server."""
        if self.server:
            logger.info("Stopping playground server")
            await self.server.shutdown()
