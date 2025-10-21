"""
FastAPI server for computer automation and control.

This module provides a web API for controlling computer interactions including
mouse, keyboard, file system operations, and accessibility features. It supports
both WebSocket and HTTP endpoints with optional authentication for cloud deployments.
"""

from fastapi import FastAPI, WebSocket, WebSocketDisconnect, Request, HTTPException, Header
from fastapi.responses import StreamingResponse, JSONResponse
from typing import List, Dict, Any, Optional, Union, Literal, cast
import uvicorn
import logging
import asyncio
import json
import traceback
import inspect
from contextlib import redirect_stdout, redirect_stderr
from io import StringIO
from .handlers.factory import HandlerFactory
import os
import aiohttp
import hashlib
import time
import platform
from fastapi.middleware.cors import CORSMiddleware

# Authentication session TTL (in seconds). Override via env var CUA_AUTH_TTL_SECONDS. Default: 60s
AUTH_SESSION_TTL_SECONDS: int = int(os.environ.get("CUA_AUTH_TTL_SECONDS", "60"))

try:
    from agent import ComputerAgent
    HAS_AGENT = True
except ImportError:
    HAS_AGENT = False

# Set up logging with more detail
logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)

# Configure WebSocket with larger message size
WEBSOCKET_MAX_SIZE = 1024 * 1024 * 10  # 10MB limit

# Configure application with WebSocket settings
app = FastAPI(
    title="Computer API",
    description="API for the Computer project",
    version="0.1.0",
    websocket_max_size=WEBSOCKET_MAX_SIZE,
)

# CORS configuration
origins = ["*"]
app.add_middleware(
    CORSMiddleware,
    allow_origins=origins,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

protocol_version = 1
try:
    from importlib.metadata import version
    package_version = version("cua-computer-server")
except Exception:
    # Fallback for cases where package is not installed or importlib.metadata is not available
    try:
        import pkg_resources
        package_version = pkg_resources.get_distribution("cua-computer-server").version
    except Exception:
        package_version = "unknown"

accessibility_handler, automation_handler, diorama_handler, file_handler = HandlerFactory.create_handlers()
handlers = {
    "version": lambda: {"protocol": protocol_version, "package": package_version},
    # App-Use commands
    "diorama_cmd": diorama_handler.diorama_cmd,
    # Accessibility commands
    "get_accessibility_tree": accessibility_handler.get_accessibility_tree,
    "find_element": accessibility_handler.find_element,
    # Shell commands
    "run_command": automation_handler.run_command,
    # File system commands
    "file_exists": file_handler.file_exists,
    "directory_exists": file_handler.directory_exists,
    "list_dir": file_handler.list_dir,
    "read_text": file_handler.read_text,
    "write_text": file_handler.write_text,
    "read_bytes": file_handler.read_bytes,
    "write_bytes": file_handler.write_bytes,
    "get_file_size": file_handler.get_file_size,
    "delete_file": file_handler.delete_file,
    "create_dir": file_handler.create_dir,
    "delete_dir": file_handler.delete_dir,
    # Mouse commands
    "mouse_down": automation_handler.mouse_down,
    "mouse_up": automation_handler.mouse_up,
    "left_click": automation_handler.left_click,
    "right_click": automation_handler.right_click,
    "double_click": automation_handler.double_click,
    "move_cursor": automation_handler.move_cursor,
    "drag_to": automation_handler.drag_to,
    "drag": automation_handler.drag,
    # Keyboard commands
    "key_down": automation_handler.key_down,
    "key_up": automation_handler.key_up,
    "type_text": automation_handler.type_text,
    "press_key": automation_handler.press_key,
    "hotkey": automation_handler.hotkey,
    # Scrolling actions
    "scroll": automation_handler.scroll,
    "scroll_down": automation_handler.scroll_down,
    "scroll_up": automation_handler.scroll_up,
    # Screen actions
    "screenshot": automation_handler.screenshot,
    "get_cursor_position": automation_handler.get_cursor_position,
    "get_screen_size": automation_handler.get_screen_size,
    # Clipboard actions
    "copy_to_clipboard": automation_handler.copy_to_clipboard,
    "set_clipboard": automation_handler.set_clipboard,
}


class AuthenticationManager:
    """
    Manages authentication for cloud deployments using container names and API keys.
    
    Provides session caching to reduce API calls and handles authentication
    against the TryCUA API service.
    """
    
    def __init__(self):
        """Initialize the authentication manager with empty session cache."""
        self.sessions: Dict[str, Dict[str, Any]] = {}
        self.container_name = os.environ.get("CONTAINER_NAME")
    
    def _hash_credentials(self, container_name: str, api_key: str) -> str:
        """
        Create a hash of container name and API key for session identification.
        
        Args:
            container_name: The container name to hash
            api_key: The API key to hash
            
        Returns:
            SHA256 hash of the combined credentials
        """
        combined = f"{container_name}:{api_key}"
        return hashlib.sha256(combined.encode()).hexdigest()
    
    def _is_session_valid(self, session_data: Dict[str, Any]) -> bool:
        """
        Check if a session is still valid based on expiration time.
        
        Args:
            session_data: Dictionary containing session validity and expiration data
            
        Returns:
            True if session is valid and not expired, False otherwise
        """
        if not session_data.get('valid', False):
            return False
        
        expires_at = session_data.get('expires_at', 0)
        return time.time() < expires_at
    
    async def auth(self, container_name: str, api_key: str) -> bool:
        """
        Authenticate container name and API key, using cached sessions when possible.
        
        Args:
            container_name: The container name to authenticate
            api_key: The API key for authentication
            
        Returns:
            True if authentication succeeds, False otherwise
        """
        # If no CONTAINER_NAME is set, always allow access (local development)
        if not self.container_name:
            logger.info("No CONTAINER_NAME set in environment. Allowing access (local development mode)")
            return True
        
        # Layer 1: VM Identity Verification
        if container_name != self.container_name:
            logger.warning(f"VM name mismatch. Expected: {self.container_name}, Got: {container_name}")
            return False
        
        # Create hash for session lookup
        session_hash = self._hash_credentials(container_name, api_key)
        
        # Check if we have a valid cached session
        if session_hash in self.sessions:
            session_data = self.sessions[session_hash]
            if self._is_session_valid(session_data):
                logger.info(f"Using cached authentication for container: {container_name}")
                return session_data['valid']
            else:
                # Remove expired session
                del self.sessions[session_hash]
        
        # No valid cached session, authenticate with API
        logger.info(f"Authenticating with TryCUA API for container: {container_name}")
        
        try:
            async with aiohttp.ClientSession() as session:
                headers = {
                    "Authorization": f"Bearer {api_key}"
                }
                
                async with session.get(
                    f"https://www.trycua.com/api/vm/auth?container_name={container_name}",
                    headers=headers,
                ) as resp:
                    is_valid = resp.status == 200 and bool((await resp.text()).strip())
                    
                    # Cache the result with configurable expiration
                    self.sessions[session_hash] = {
                        'valid': is_valid,
                        'expires_at': time.time() + AUTH_SESSION_TTL_SECONDS
                    }
                    
                    if is_valid:
                        logger.info(f"Authentication successful for container: {container_name}")
                    else:
                        logger.warning(f"Authentication failed for container: {container_name}. Status: {resp.status}")
                    
                    return is_valid
        
        except aiohttp.ClientError as e:
            logger.error(f"Failed to validate API key with TryCUA API: {str(e)}")
            # Cache failed result to avoid repeated requests
            self.sessions[session_hash] = {
                'valid': False,
                'expires_at': time.time() + AUTH_SESSION_TTL_SECONDS
            }
            return False
        except Exception as e:
            logger.error(f"Unexpected error during authentication: {str(e)}")
            # Cache failed result to avoid repeated requests
            self.sessions[session_hash] = {
                'valid': False,
                'expires_at': time.time() + AUTH_SESSION_TTL_SECONDS
            }
            return False


class ConnectionManager:
    """
    Manages WebSocket connections for the server.
    
    Handles connecting and disconnecting WebSocket clients.
    """
    
    def __init__(self):
        """Initialize with empty connection list."""
        self.active_connections: List[WebSocket] = []

    async def connect(self, websocket: WebSocket):
        """
        Accept and register a new WebSocket connection.
        
        Args:
            websocket: The WebSocket connection to accept
        """
        await websocket.accept()
        self.active_connections.append(websocket)

    def disconnect(self, websocket: WebSocket):
        """
        Remove a WebSocket connection from active connections.
        
        Args:
            websocket: The WebSocket connection to remove
        """
        self.active_connections.remove(websocket)


manager = ConnectionManager()
auth_manager = AuthenticationManager()

@app.get("/status")
async def status():
    """
    Get server status including OS type and available features.
    
    Returns:
        Dict containing status, OS type, and available features
    """
    sys = platform.system().lower()
    # get os type
    if "darwin" in sys or sys == "macos" or sys == "mac":
        os_type = "macos"
    elif "windows" in sys:
        os_type = "windows"
    else:
        os_type = "linux"
    # get computer-server features
    features = []
    if HAS_AGENT:
        features.append("agent")
    return {"status": "ok", "os_type": os_type, "features": features}

@app.websocket("/ws", name="websocket_endpoint")
async def websocket_endpoint(websocket: WebSocket):
    """
    Main WebSocket endpoint for handling computer automation commands.
    
    Supports authentication for cloud deployments and processes commands
    through registered handlers.
    
    Args:
        websocket: The WebSocket connection
    """
    global handlers

    # WebSocket message size is configured at the app or endpoint level, not on the instance
    await manager.connect(websocket)
    
    # Check if CONTAINER_NAME is set (indicating cloud provider)
    server_container_name = os.environ.get("CONTAINER_NAME")
    
    # If cloud provider, perform authentication handshake
    if server_container_name:
        try:
            logger.info(f"Cloud provider detected. CONTAINER_NAME: {server_container_name}. Waiting for authentication...")
            
            # Wait for authentication message
            auth_data = await websocket.receive_json()
            
            # Validate auth message format
            if auth_data.get("command") != "authenticate":
                await websocket.send_json({
                    "success": False,
                    "error": "First message must be authentication"
                })
                await websocket.close()
                manager.disconnect(websocket)
                return
            
            # Extract credentials
            client_api_key = auth_data.get("params", {}).get("api_key")
            client_container_name = auth_data.get("params", {}).get("container_name")
            
            # Validate credentials using AuthenticationManager
            if not client_api_key:
                await websocket.send_json({
                    "success": False,
                    "error": "API key required"
                })
                await websocket.close()
                manager.disconnect(websocket)
                return
            
            if not client_container_name:
                await websocket.send_json({
                    "success": False,
                    "error": "Container name required"
                })
                await websocket.close()
                manager.disconnect(websocket)
                return
            
            # Use AuthenticationManager for validation
            is_authenticated = await auth_manager.auth(client_container_name, client_api_key)
            if not is_authenticated:
                await websocket.send_json({
                    "success": False,
                    "error": "Authentication failed"
                })
                await websocket.close()
                manager.disconnect(websocket)
                return
            
            logger.info(f"Authentication successful for VM: {client_container_name}")
            await websocket.send_json({
                "success": True,
                "message": "Authentication successful"
            })
        
        except Exception as e:
            logger.error(f"Error during authentication handshake: {str(e)}")
            await websocket.send_json({
                "success": False,
                "error": "Authentication failed"
            })
            await websocket.close()
            manager.disconnect(websocket)
            return

    try:
        while True:
            try:
                data = await websocket.receive_json()
                command = data.get("command")
                params = data.get("params", {})

                if command not in handlers:
                    await websocket.send_json(
                        {"success": False, "error": f"Unknown command: {command}"}
                    )
                    continue

                try:
                    # Filter params to only include those accepted by the handler function
                    handler_func = handlers[command]
                    sig = inspect.signature(handler_func)
                    filtered_params = {k: v for k, v in params.items() if k in sig.parameters}
                    
                    # Handle both sync and async functions
                    if asyncio.iscoroutinefunction(handler_func):
                        result = await handler_func(**filtered_params)
                    else:
                        # Run sync functions in thread pool to avoid blocking event loop
                        result = await asyncio.to_thread(handler_func, **filtered_params)
                    await websocket.send_json({"success": True, **result})
                except Exception as cmd_error:
                    logger.error(f"Error executing command {command}: {str(cmd_error)}")
                    logger.error(traceback.format_exc())
                    await websocket.send_json({"success": False, "error": str(cmd_error)})

            except WebSocketDisconnect:
                raise
            except json.JSONDecodeError as json_err:
                logger.error(f"JSON decode error: {str(json_err)}")
                await websocket.send_json(
                    {"success": False, "error": f"Invalid JSON: {str(json_err)}"}
                )
            except Exception as loop_error:
                logger.error(f"Error in message loop: {str(loop_error)}")
                logger.error(traceback.format_exc())
                await websocket.send_json({"success": False, "error": str(loop_error)})

    except WebSocketDisconnect:
        logger.info("Client disconnected")
        manager.disconnect(websocket)
    except Exception as e:
        logger.error(f"Fatal error in websocket connection: {str(e)}")
        logger.error(traceback.format_exc())
        try:
            await websocket.close()
        except:
            pass
        manager.disconnect(websocket)

@app.post("/cmd")
async def cmd_endpoint(
    request: Request,
    container_name: Optional[str] = Header(None, alias="X-Container-Name"),
    api_key: Optional[str] = Header(None, alias="X-API-Key")
):
    """
    Backup endpoint for when WebSocket connections fail.
    Accepts commands via HTTP POST with streaming response.
    
    Args:
        request: The HTTP request object
        container_name: Container name for cloud authentication (from header)
        api_key: API key for cloud authentication (from header)
    
    Headers:
        X-Container-Name: Container name for cloud authentication
        X-API-Key: API key for cloud authentication
    
    Body:
        {
            "command": "command_name",
            "params": {...}
        }
    
    Returns:
        StreamingResponse with command execution results
    """
    global handlers
    
    # Parse request body
    try:
        body = await request.json()
        command = body.get("command")
        params = body.get("params", {})
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON body: {str(e)}")
    
    if not command:
        raise HTTPException(status_code=400, detail="Command is required")
    
    # Check if CONTAINER_NAME is set (indicating cloud provider)
    server_container_name = os.environ.get("CONTAINER_NAME")
    
    # If cloud provider, perform authentication
    if server_container_name:
        logger.info(f"Cloud provider detected. CONTAINER_NAME: {server_container_name}. Performing authentication...")
        
        # Validate required headers
        if not container_name:
            raise HTTPException(status_code=401, detail="Container name required")
        
        if not api_key:
            raise HTTPException(status_code=401, detail="API key required")
        
        # Validate with AuthenticationManager
        is_authenticated = await auth_manager.auth(container_name, api_key)
        if not is_authenticated:
            raise HTTPException(status_code=401, detail="Authentication failed")
    
    if command not in handlers:
        raise HTTPException(status_code=400, detail=f"Unknown command: {command}")
    
    async def generate_response():
        """
        Generate streaming response for the command execution.
        
        Yields:
            JSON-formatted response data as server-sent events
        """
        try:
            # Filter params to only include those accepted by the handler function
            handler_func = handlers[command]
            sig = inspect.signature(handler_func)
            filtered_params = {k: v for k, v in params.items() if k in sig.parameters}
            
            # Handle both sync and async functions
            if asyncio.iscoroutinefunction(handler_func):
                result = await handler_func(**filtered_params)
            else:
                # Run sync functions in thread pool to avoid blocking event loop
                result = await asyncio.to_thread(handler_func, **filtered_params)
            
            # Stream the successful result
            response_data = {"success": True, **result}
            yield f"data: {json.dumps(response_data)}\n\n"
            
        except Exception as cmd_error:
            logger.error(f"Error executing command {command}: {str(cmd_error)}")
            logger.error(traceback.format_exc())
            
            # Stream the error result
            error_data = {"success": False, "error": str(cmd_error)}
            yield f"data: {json.dumps(error_data)}\n\n"
    
    return StreamingResponse(
        generate_response(),
        media_type="text/plain",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
        }
    )

@app.post("/responses")
async def agent_response_endpoint(
    request: Request,
    api_key: Optional[str] = Header(None, alias="X-API-Key"),
):
    """
    Minimal proxy to run ComputerAgent for up to 2 turns.

    Args:
        request: The HTTP request object
        api_key: API key for authentication (from header)

    Security:
        If CONTAINER_NAME is set on the server, require X-API-Key
        and validate using AuthenticationManager unless CUA_ENABLE_PUBLIC_PROXY is true.

    Body JSON:
        {
          "model": "...",                 # required
          "input": "... or messages[]",   # required
          "agent_kwargs": { ... },         # optional, passed directly to ComputerAgent
          "env": { ... }                   # optional env overrides for agent
        }
    
    Returns:
        JSONResponse with agent execution results
    """
    if not HAS_AGENT:
        raise HTTPException(status_code=501, detail="ComputerAgent not available")
    
    # Authenticate via AuthenticationManager if running in cloud (CONTAINER_NAME set)
    container_name = os.environ.get("CONTAINER_NAME")
    if container_name:
        is_public = os.environ.get("CUA_ENABLE_PUBLIC_PROXY", "").lower().strip() in ["1", "true", "yes", "y", "on"]
        if not is_public:
            if not api_key:
                raise HTTPException(status_code=401, detail="Missing AGENT PROXY auth headers")
            ok = await auth_manager.auth(container_name, api_key)
            if not ok:
                raise HTTPException(status_code=401, detail="Unauthorized")

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
        """Context manager for temporarily overriding environment variables."""
        
        def __init__(self, overrides: Dict[str, str]):
            """
            Initialize with environment variable overrides.
            
            Args:
                overrides: Dictionary of environment variables to override
            """
            self.overrides = overrides
            self._original: Dict[str, Optional[str]] = {}
            
        def __enter__(self):
            """Apply environment variable overrides."""
            for k, v in (self.overrides or {}).items():
                self._original[k] = os.environ.get(k)
                os.environ[k] = str(v)
                
        def __exit__(self, exc_type, exc, tb):
            """Restore original environment variables."""
            for k, old in self._original.items():
                if old is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = old

    # Convert input to messages
    def _to_messages(data: Union[str, List[Dict[str, Any]]]) -> List[Dict[str, Any]]:
        """
        Convert string or message list to standard message format.
        
        Args:
            data: Input data as string or list of message dictionaries
            
        Returns:
            List of message dictionaries in standard format
        """
        if isinstance(data, str):
            return [{"role": "user", "content": data}]
        if isinstance(data, list):
            return data

    messages = _to_messages(input_data)

    # Define a direct computer tool that implements the AsyncComputerHandler protocol
    # and delegates to our existing automation/file/accessibility handlers.
    from agent.computers import AsyncComputerHandler  # runtime-checkable Protocol

    class DirectComputer(AsyncComputerHandler):
        """
        Direct computer interface that delegates to existing handlers.
        
        Implements the AsyncComputerHandler protocol for agent integration.
        """
        
        def __init__(self):
            """Initialize with module-scope handler singletons."""
            # use module-scope handler singletons created by HandlerFactory
            self._auto = automation_handler
            self._file = file_handler
            self._access = accessibility_handler

        async def get_environment(self) -> Literal["windows", "mac", "linux", "browser"]:
            """
            Get the current operating system environment.
            
            Returns:
                The operating system type as a string literal
            """
            sys = platform.system().lower()
            if "darwin" in sys or sys in ("macos", "mac"):
                return "mac"
            if "windows" in sys:
                return "windows"
            return "linux"

        async def get_dimensions(self) -> tuple[int, int]:
            """
            Get screen dimensions as width, height tuple.
            
            Returns:
                Tuple of (width, height) in pixels
            """
            size = await self._auto.get_screen_size()
            return size["width"], size["height"]

        async def screenshot(self) -> str:
            """
            Take a screenshot and return as base64 encoded string.
            
            Returns:
                Base64 encoded screenshot image data
            """
            img_b64 = await self._auto.screenshot()
            return img_b64["image_data"]

        async def click(self, x: int, y: int, button: str = "left") -> None:
            """
            Click at the specified coordinates with the given button.
            
            Args:
                x: X coordinate for the click
                y: Y coordinate for the click
                button: Mouse button to use ("left" or "right")
            """
            if button == "left":
                await self._auto.left_click(x, y)
            elif button == "right":
                await self._auto.right_click(x, y)
            else:
                await self._auto.left_click(x, y)

        async def double_click(self, x: int, y: int) -> None:
            """
            Double-click at the specified coordinates.
            
            Args:
                x: X coordinate for the double-click
                y: Y coordinate for the double-click
            """
            await self._auto.double_click(x, y)

        async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
            """
            Scroll at the specified position with given scroll amounts.
            
            Args:
                x: X coordinate where to scroll
                y: Y coordinate where to scroll
                scroll_x: Horizontal scroll amount
                scroll_y: Vertical scroll amount
            """
            await self._auto.move_cursor(x, y)
            await self._auto.scroll(scroll_x, scroll_y)

        async def type(self, text: str) -> None:
            """
            Type the specified text.
            
            Args:
                text: Text to type
            """
            await self._auto.type_text(text)

        async def wait(self, ms: int = 1000) -> None:
            """
            Wait for the specified number of milliseconds.
            
            Args:
                ms: Number of milliseconds to wait
            """
            await asyncio.sleep(ms / 1000.0)

        async def move(self, x: int, y: int) -> None:
            """
            Move cursor to the specified coordinates.
            
            Args:
                x: X coordinate to move to
                y: Y coordinate to move to
            """
            await self._auto.move_cursor(x, y)

        async def keypress(self, keys: Union[List[str], str]) -> None:
            """
            Press the specified key or key combination.
            
            Args:
                keys: Single key or list of keys to press
            """
            if isinstance(keys, str):
                parts = keys.replace("-", "+").split("+") if len(keys) > 1 else [keys]
            else:
                parts = keys
            if len(parts) == 1:
                await self._auto.press_key(parts[0])
            else:
                await self._auto.hotkey(parts)

        async def drag(self, path: List[Dict[str, int]]) -> None:
            """
            Drag along the specified path of coordinates.
            
            Args:
                path: List of coordinate dictionaries with "x" and "y" keys
            """
            if not path:
                return
            start = path[0]
            await self._auto.mouse_down(start["x"], start["y"])
            for pt in path[1:]:
                await self._auto.move_cursor(pt["x"], pt["y"]) 
            end = path[-1]
            await self._auto.mouse_up(end["x"], end["y"]) 

        async def get_current_url(self) -> str:
            """
            Get current URL (not available in this server context).
            
            Returns:
                Empty string as URLs are not available in desktop context
            """
            # Not available in this server context
            return ""

        async def left_mouse_down(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
            """
            Press left mouse button down at specified coordinates.
            
            Args:
                x: X coordinate for mouse down (optional)
                y: Y coordinate for mouse down (optional)
            """
            await self._auto.mouse_down(x, y, button="left")

        async def left_mouse_up(self, x: Optional[int] = None, y: Optional[int] = None) -> None:
            """
            Release left mouse button at specified coordinates.
            
            Args:
                x: X coordinate for mouse up (optional)
                y: Y coordinate for mouse up (optional)
            """
            await self._auto.mouse_up(x, y, button="left")

    # # Inline image URLs to base64
    # import base64, mimetypes, requests
    # # Use a browser-like User-Agent to avoid 403s from some CDNs (e.g., Wikimedia)
    # HEADERS = {
    #     "User-Agent": (
    #         "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
    #         "AppleWebKit/537.36 (KHTML, like Gecko) "
    #         "Chrome/124.0.0.0 Safari/537.36"
    #     )
    # }
    # def _to_data_url(content_bytes: bytes, url: str, resp: requests.Response) -> str:
    #     ctype = resp.headers.get("Content-Type") or mimetypes.guess_type(url)[0] or "application/octet-stream"
    #     b64 = base64.b64encode(content_bytes).decode("utf-8")
    #     return f"data:{ctype};base64,{b64}"
    # def inline_image_urls(messages):
    #     # messages: List[{"role": "...","content":[...]}]
    #     out = []
    #     for m in messages:
    #         if not isinstance(m.get("content"), list):
    #             out.append(m)
    #             continue
    #         new_content = []
    #         for part in (m.get("content") or []):
    #             if part.get("type") == "input_image" and (url := part.get("image_url")):
    #                 resp = requests.get(url, headers=HEADERS, timeout=30)
    #                 resp.raise_for_status()
    #                 new_content.append({
    #                     "type": "input_image",
    #                     "image_url": _to_data_url(resp.content, url, resp)
    #                 })
    #             else:
    #                 new_content.append(part)
    #         out.append({**m, "content": new_content})
    #     return out
    # messages = inline_image_urls(messages)

    error = None

    with _EnvOverride(env_overrides):
        # Prepare tools: if caller did not pass tools, inject our DirectComputer
        tools = agent_kwargs.get("tools")
        if not tools:
            tools = [DirectComputer()]
            agent_kwargs = {**agent_kwargs, "tools": tools}
        # Instantiate agent with our tools
        agent = ComputerAgent(model=model, **agent_kwargs)  # type: ignore[arg-type]

        total_output: List[Any] = []
        total_usage: Dict[str, Any] = {}

        pending_computer_call_ids = set()
        try:
            async for result in agent.run(messages):
                total_output += result["output"]
                # Try to collect usage if present
                if isinstance(result, dict) and "usage" in result and isinstance(result["usage"], dict):
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
        "status": "completed" if not error else "failed"
    }

    # CORS: allow any origin
    headers = {
        "Cache-Control": "no-cache",
        "Connection": "keep-alive",
    }

    return JSONResponse(content=payload, headers=headers)


if __name__ == "__main__":
    """Run the FastAPI server when executed directly."""
    uvicorn.run(app, host="0.0.0.0", port=8000)
