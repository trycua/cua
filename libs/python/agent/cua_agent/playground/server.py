"""Playground server implementation for Cua agents."""

import asyncio
import logging
import os
import platform
import re
import socket
import traceback
import webbrowser
from typing import Any, Dict, Iterable, List, Optional, Tuple, Union
from urllib.parse import quote

import uvicorn
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

logger = logging.getLogger(__name__)

DEFAULT_HOST = "127.0.0.1"

# Browser origins served from this machine (any port) are always allowed.
_LOOPBACK_ORIGIN_RE = re.compile(r"https?://(localhost|127\.0\.0\.1|\[::1\])(:\d+)?")

# agent_kwargs a request may set by default: harmless tuning knobs only.
_SAFE_AGENT_KWARGS = frozenset(
    {
        "only_n_most_recent_images",
        "use_prompt_caching",
        "max_retries",
        "screenshot_delay",
        "instructions",
        "verbosity",
        "temperature",
        "top_p",
        "max_tokens",
    }
)

# agent_kwargs a request can never set, even with allow_request_overrides:
# they load code, write to disk or swap the agent's tools and loop.
_BLOCKED_AGENT_KWARGS = frozenset(
    {
        "trust_remote_code",
        "trajectory_dir",
        "tools",
        "callbacks",
        "custom_loop",
    }
)


def _is_loopback_host(host: str) -> bool:
    return host in ("127.0.0.1", "localhost", "::1")


class PlaygroundServer:
    """Playground server for running Cua agents via HTTP API.

    Security model: the server has no authentication, so it is only meant to be
    reachable from the machine it runs on. It binds to loopback by default, only
    answers browser origins served from loopback (plus any ``allowed_origins``
    you list), never writes request data into ``os.environ``, and ignores any
    request field that could redirect the agent's model traffic (``api_base``,
    ``api_key``, ...) unless ``allow_request_overrides=True`` is set.
    """

    def __init__(
        self,
        agent_instance=None,
        *,
        allowed_origins: Optional[Iterable[str]] = None,
        allow_request_overrides: bool = False,
    ):
        """
        Initialize the playground server.

        Args:
            agent_instance: Optional pre-configured agent instance to use
            allowed_origins: Extra browser origins (for example
                ``"https://dashboard.example.com"``) allowed to call the API, on
                top of loopback origins. ``"*"`` is not accepted.
            allow_request_overrides: Let a request set ``agent_kwargs`` that
                point the agent at another model endpoint or credentials
                (``api_base``, ``base_url``, ``api_key``, ...). Off by default.
                Only turn this on if you trust every client that can reach
                the server.
        """
        self.agent_instance = agent_instance
        self.allow_request_overrides = allow_request_overrides
        self.allowed_origins = self._normalize_origins(allowed_origins)
        self.app = FastAPI(
            title="Cua Playground Server",
            description="Playground server for Cua agents",
            version="0.1.0",
        )
        self._setup_middleware()
        self._setup_routes()
        self.server = None
        self.port = None

    @staticmethod
    def _normalize_origins(origins: Optional[Iterable[str]]) -> List[str]:
        normalized: List[str] = []
        for origin in origins or ():
            origin = origin.strip().rstrip("/").lower()
            if not origin:
                continue
            if "*" in origin:
                raise ValueError(
                    "Wildcard origins are not allowed for the playground server; "
                    "list each origin explicitly."
                )
            normalized.append(origin)
        return normalized

    def _is_origin_allowed(self, origin: str) -> bool:
        return (
            _LOOPBACK_ORIGIN_RE.fullmatch(origin) is not None
            or origin.rstrip("/").lower() in self.allowed_origins
        )

    def _setup_middleware(self):
        """Setup origin checks and CORS (added last, so it runs first)."""

        @self.app.middleware("http")
        async def reject_foreign_origins(request: Request, call_next):
            # CORS only stops a page from reading a response; a cross-origin
            # "simple" POST is still executed. Browsers always send Origin on
            # cross-origin requests, so refuse them before any route runs.
            origin = request.headers.get("origin")
            if origin is not None and not self._is_origin_allowed(origin):
                return JSONResponse(status_code=403, content={"detail": "Origin not allowed"})
            return await call_next(request)

        self.app.add_middleware(
            CORSMiddleware,
            allow_origins=self.allowed_origins,
            allow_origin_regex=_LOOPBACK_ORIGIN_RE.pattern,
            allow_credentials=False,
            allow_methods=["GET", "POST", "OPTIONS"],
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
              "agent_kwargs": { ... },         # optional tuning options for ComputerAgent
              "env": { ... }                   # ignored; kept so older clients still work
            }

            ``env`` is never applied. ``agent_kwargs`` is limited to harmless tuning
            options; anything else (``api_base``, ``api_key``, ...) is ignored unless
            the server was created with ``allow_request_overrides=True``.
            """
            # Import here to avoid circular imports
            try:
                from cua_agent import ComputerAgent
            except ImportError:
                raise HTTPException(status_code=501, detail="ComputerAgent not available")

            # Parse request body
            try:
                body = await request.json()
            except Exception as e:
                raise HTTPException(status_code=400, detail=f"Invalid JSON body: {str(e)}")
            if not isinstance(body, dict):
                raise HTTPException(status_code=400, detail="Body must be a JSON object")

            model = body.get("model")
            input_data = body.get("input")
            if not model or input_data is None:
                raise HTTPException(status_code=400, detail="'model' and 'input' are required")

            raw_agent_kwargs = body.get("agent_kwargs") or {}
            if not isinstance(raw_agent_kwargs, dict):
                raise HTTPException(status_code=400, detail="'agent_kwargs' must be an object")
            agent_kwargs, ignored = self._filter_agent_kwargs(raw_agent_kwargs)

            # Never write request data into the process environment.
            if body.get("env"):
                ignored.append("env")
            if ignored:
                logger.warning(
                    "Ignored request fields that the playground server does not accept: %s",
                    ", ".join(sorted(set(ignored))),
                )

            # Convert input to messages
            def _to_messages(data: Union[str, List[Dict[str, Any]]]) -> List[Dict[str, Any]]:
                if isinstance(data, str):
                    return [{"role": "user", "content": data}]
                if isinstance(data, list):
                    return data
                return []

            messages = _to_messages(input_data)

            error = None

            # Use pre-configured agent only if model matches, otherwise create new one
            # This ensures the model picker in the UI is respected
            if self.agent_instance and self.agent_instance.model == model:
                agent = self.agent_instance
            else:
                # Model changed or no pre-configured agent - create new agent with requested model
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
            if ignored:
                payload["ignored_request_fields"] = sorted(set(ignored))

            headers = {
                "Cache-Control": "no-cache",
                "Connection": "keep-alive",
            }

            return JSONResponse(content=payload, headers=headers)

    def _filter_agent_kwargs(self, requested: Dict[str, Any]) -> Tuple[Dict[str, Any], List[str]]:
        """Split request agent_kwargs into (accepted, ignored key names)."""
        accepted: Dict[str, Any] = {}
        ignored: List[str] = []
        for key, value in requested.items():
            if key in _BLOCKED_AGENT_KWARGS:
                ignored.append(f"agent_kwargs.{key}")
            elif key in _SAFE_AGENT_KWARGS or self.allow_request_overrides:
                accepted[key] = value
            else:
                ignored.append(f"agent_kwargs.{key}")
        return accepted, ignored

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

    async def start_async(
        self,
        port: Optional[int] = None,
        open_browser: bool = False,
        host: str = DEFAULT_HOST,
    ):
        """
        Start the playground server asynchronously.

        Args:
            port: Port to run the server on. If None, finds an available port.
            open_browser: Whether to open the browser automatically.
            host: Interface to bind. Defaults to loopback. Pass "0.0.0.0" only if
                you really want other machines to reach this unauthenticated server.
        """
        if port is None:
            port = self._find_available_port()

        self.port = port
        base_url = f"http://localhost:{port}"

        if not _is_loopback_host(host):
            logger.warning(
                "Playground server is binding to %s and has no authentication; "
                "anyone who can reach this address can run the agent.",
                host,
            )
        logger.info(f"Starting playground server on {base_url} (bound to {host})")

        if open_browser:
            # Construct the playground URL
            encoded_host = quote(base_url, safe="")
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
            host=host,
            port=port,
            log_level="info",
        )
        self.server = uvicorn.Server(config)
        await self.server.serve()

    def start(
        self,
        port: Optional[int] = None,
        open_browser: bool = False,
        host: str = DEFAULT_HOST,
    ):
        """
        Start the playground server (blocking).

        Args:
            port: Port to run the server on. If None, finds an available port.
            open_browser: Whether to open the browser automatically.
            host: Interface to bind. Defaults to loopback (see ``start_async``).
        """
        # Check if there's already a running event loop
        try:
            loop = asyncio.get_running_loop()
            # If we're in an async context, schedule as a task
            import threading

            # Run the server in a separate thread to avoid blocking
            server_thread = threading.Thread(
                target=self._run_in_new_loop,
                args=(port, open_browser, host),
                daemon=True,
            )
            server_thread.start()

            # Give the server a moment to start and open browser
            import time

            time.sleep(1)

        except RuntimeError:
            # No running loop, can use asyncio.run() safely
            asyncio.run(self.start_async(port=port, open_browser=open_browser, host=host))

    def _run_in_new_loop(
        self,
        port: Optional[int] = None,
        open_browser: bool = False,
        host: str = DEFAULT_HOST,
    ):
        """Helper to run server in a new event loop (for threading)."""
        new_loop = asyncio.new_event_loop()
        asyncio.set_event_loop(new_loop)
        try:
            new_loop.run_until_complete(
                self.start_async(port=port, open_browser=open_browser, host=host)
            )
        finally:
            new_loop.close()

    async def stop(self):
        """Stop the playground server."""
        if self.server:
            logger.info("Stopping playground server")
            await self.server.shutdown()
