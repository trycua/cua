"""FastAPI server wrapping cua_bench Environment instances.

This server provides a REST API for managing a pool of Environment instances,
enabling parallel environment execution for RL training.

Usage:
    OSGYM_ALLOWED_IPS=127.0.0.1 python -m cua_bench.workers.worker_server --port 8001

Environment Variables:
    OSGYM_ALLOWED_IPS: Comma-separated list of allowed IP addresses (default: 127.0.0.1)
"""

import argparse
import asyncio
import base64
import os
import threading
import time
from dataclasses import asdict, dataclass
from typing import Any, Dict, List, Optional

# Import cua_bench components
from cua_bench import make
from cua_bench.types import (
    Action,
    ClickAction,
    DoneAction,
    DoubleClickAction,
    DragAction,
    HotkeyAction,
    KeyAction,
    MiddleClickAction,
    MoveToAction,
    RightClickAction,
    ScrollAction,
    TypeAction,
    WaitAction,
)
from fastapi import FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

# Maximum number of environments per server
MAX_ENVS = 2

# Default timeout for inactive environments (seconds)
DEFAULT_TIMEOUT = 300


@dataclass
class EnvState:
    """State of an environment instance."""

    env: Any  # Environment instance
    env_path: str
    task_index: int
    split: str
    timeout: int
    last_accessed: float
    instruction: str = ""


# Global state
env_lock = threading.Lock()
available_envs: List[int] = list(range(MAX_ENVS - 1, -1, -1))  # [1, 0] for MAX_ENVS=2
active_envs: List[int] = []
env_map: Dict[int, EnvState] = {}

# FastAPI app
app = FastAPI(title="CUA-Bench Worker Server", version="0.1.0")

# Add CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# --- IP Filtering Middleware ---
@app.middleware("http")
async def ip_filter_middleware(request: Request, call_next):
    """Filter requests by IP address."""
    allowed_ips = os.environ.get("OSGYM_ALLOWED_IPS", "127.0.0.1").split(",")
    allowed_ips = [ip.strip() for ip in allowed_ips if ip.strip()]

    client_ip = request.client.host if request.client else "unknown"

    # Allow localhost variants (including "testclient" for FastAPI TestClient)
    localhost_variants = ["127.0.0.1", "::1", "localhost", "testclient"]
    if client_ip in localhost_variants:
        client_ip = "127.0.0.1"

    if allowed_ips and client_ip not in allowed_ips:
        raise HTTPException(
            status_code=403,
            detail=f"Access denied: IP {client_ip} not in allowed list",
        )

    return await call_next(request)


# --- Request/Response Models ---
class ResetRequest(BaseModel):
    """Request body for /reset endpoint."""

    env_path: str
    task_index: int = 0
    split: str = "train"
    timeout: int = DEFAULT_TIMEOUT


class StepRequest(BaseModel):
    """Request body for /step endpoint."""

    action: Dict[str, Any]  # Serialized Action
    env_id: int


class ShutdownRequest(BaseModel):
    """Request body for /shutdown endpoint."""

    env_id: Optional[int] = None  # If None, shut down all envs


class ResetResponse(BaseModel):
    """Response body for /reset endpoint."""

    screenshot: str  # Base64-encoded screenshot
    instruction: str
    env_id: int


class StepResponse(BaseModel):
    """Response body for /step endpoint."""

    screenshot: str  # Base64-encoded screenshot
    reward: float
    done: bool


class ScreenshotResponse(BaseModel):
    """Response body for /screenshot endpoint."""

    screenshot: str  # Base64-encoded screenshot
    env_id: int


class HealthResponse(BaseModel):
    """Response body for /health endpoint."""

    status: str
    available_envs: int
    active_envs: int
    max_envs: int


# --- Helper Functions ---
def _get_available_env(timeout: int) -> int:
    """Get an available environment slot from the pool."""
    with env_lock:
        if not available_envs:
            raise HTTPException(
                status_code=503,
                detail=f"No available environments. Max capacity: {MAX_ENVS}",
            )
        env_id = available_envs.pop()
        active_envs.append(env_id)
        return env_id


def _release_env(env_id_or_all: int | str) -> None:
    """Release an environment back to the pool."""
    with env_lock:
        if env_id_or_all == "all":
            # Release all active envs
            for eid in list(active_envs):
                if eid in env_map:
                    state = env_map.pop(eid)
                    # Close the environment
                    try:
                        asyncio.create_task(_close_env_async(state.env))
                    except Exception:
                        pass
                active_envs.remove(eid)
                available_envs.append(eid)
        else:
            env_id = int(env_id_or_all)
            if env_id in active_envs:
                if env_id in env_map:
                    state = env_map.pop(env_id)
                    # Close the environment
                    try:
                        asyncio.create_task(_close_env_async(state.env))
                    except Exception:
                        pass
                active_envs.remove(env_id)
                available_envs.append(env_id)


async def _close_env_async(env: Any) -> None:
    """Close an environment asynchronously."""
    try:
        await env.close()
    except Exception:
        pass


def _get_env(env_id: int) -> EnvState:
    """Get an environment by ID."""
    with env_lock:
        if env_id not in env_map:
            raise HTTPException(
                status_code=404,
                detail=f"Environment {env_id} not found",
            )
        state = env_map[env_id]
        state.last_accessed = time.time()
        return state


def deserialize_action(action_dict: Dict[str, Any]) -> Action:
    """Deserialize an action from a dictionary.

    Supports two formats:
    1. {"type": "ClickAction", "x": 100, "y": 200}
    2. {"action_type": "click", "x": 100, "y": 200}
    """
    action_type = action_dict.get("type") or action_dict.get("action_type", "")

    # Normalize action type
    action_type_lower = action_type.lower().replace("_", "").replace("action", "")

    if action_type_lower == "click" or action_type == "ClickAction":
        return ClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
    elif action_type_lower == "rightclick" or action_type == "RightClickAction":
        return RightClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
    elif action_type_lower == "doubleclick" or action_type == "DoubleClickAction":
        return DoubleClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
    elif action_type_lower == "middleclick" or action_type == "MiddleClickAction":
        return MiddleClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
    elif action_type_lower == "drag" or action_type == "DragAction":
        return DragAction(
            from_x=int(action_dict["from_x"]),
            from_y=int(action_dict["from_y"]),
            to_x=int(action_dict["to_x"]),
            to_y=int(action_dict["to_y"]),
            duration=float(action_dict.get("duration", 1.0)),
        )
    elif action_type_lower == "moveto" or action_type == "MoveToAction":
        return MoveToAction(
            x=int(action_dict["x"]),
            y=int(action_dict["y"]),
            duration=float(action_dict.get("duration", 0.0)),
        )
    elif action_type_lower == "scroll" or action_type == "ScrollAction":
        return ScrollAction(
            direction=action_dict.get("direction", "up"),
            amount=int(action_dict.get("amount", 100)),
        )
    elif action_type_lower == "type" or action_type == "TypeAction":
        return TypeAction(text=str(action_dict["text"]))
    elif action_type_lower == "key" or action_type == "KeyAction":
        return KeyAction(key=str(action_dict["key"]))
    elif action_type_lower == "hotkey" or action_type == "HotkeyAction":
        return HotkeyAction(keys=list(action_dict["keys"]))
    elif action_type_lower == "wait" or action_type == "WaitAction":
        return WaitAction(seconds=float(action_dict.get("seconds", 1.0)))
    elif action_type_lower == "done" or action_type == "DoneAction":
        return DoneAction()
    else:
        raise ValueError(f"Unknown action type: {action_type}")


def serialize_action(action: Action) -> Dict[str, Any]:
    """Serialize an action to a dictionary."""
    result = asdict(action)
    result["type"] = type(action).__name__
    return result


# --- Timeout Management ---
def _timeout_checker():
    """Background thread that releases inactive environments."""
    while True:
        time.sleep(30)  # Check every 30 seconds
        current_time = time.time()

        with env_lock:
            for env_id in list(active_envs):
                if env_id in env_map:
                    state = env_map[env_id]
                    if current_time - state.last_accessed > state.timeout:
                        # Release timed out environment
                        _release_env(env_id)


# Start timeout checker thread
_timeout_thread = threading.Thread(target=_timeout_checker, daemon=True)
_timeout_thread.start()


# --- API Endpoints ---
@app.post("/reset", response_model=ResetResponse)
async def reset(request: ResetRequest):
    """Reset environment to a task.

    This endpoint:
    1. Releases all active environments first
    2. Gets an available environment slot from the pool
    3. Creates/resets the environment to the specified task
    4. Returns the initial screenshot and task instruction
    """
    # Release all active envs first (as per incus pattern)
    _release_env("all")

    # Get available env from pool
    env_id = _get_available_env(request.timeout)

    try:
        # Create environment
        env = make(request.env_path, split=request.split)

        # Reset to task
        screenshot, task_cfg = await env.reset(task_id=request.task_index)

        # Get instruction from task config
        instruction = ""
        if hasattr(task_cfg, "description"):
            instruction = task_cfg.description
        elif isinstance(task_cfg, dict) and "description" in task_cfg:
            instruction = task_cfg["description"]

        # Store environment state
        with env_lock:
            env_map[env_id] = EnvState(
                env=env,
                env_path=request.env_path,
                task_index=request.task_index,
                split=request.split,
                timeout=request.timeout,
                last_accessed=time.time(),
                instruction=instruction,
            )

        # Encode screenshot
        screenshot_b64 = base64.b64encode(screenshot).decode("utf-8")

        return ResetResponse(
            screenshot=screenshot_b64,
            instruction=instruction,
            env_id=env_id,
        )

    except Exception as e:
        # Release env on error
        _release_env(env_id)
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/step", response_model=StepResponse)
async def step(request: StepRequest):
    """Execute an action in the environment.

    Returns the resulting screenshot, reward (if done), and done flag.
    """
    state = _get_env(request.env_id)

    try:
        # Deserialize action
        action = deserialize_action(request.action)

        # Execute action
        screenshot = await state.env.step(action)

        # Check if done
        done = isinstance(action, DoneAction)
        reward = 0.0

        if done:
            # Run evaluation
            try:
                result = await state.env.evaluate()
                if isinstance(result, (int, float)):
                    reward = float(result)
                elif isinstance(result, list) and len(result) > 0:
                    reward = float(result[0])
                elif isinstance(result, dict) and "reward" in result:
                    reward = float(result["reward"])
            except Exception:
                # Evaluation may not be implemented
                pass

            # Release environment
            _release_env(request.env_id)

        # Encode screenshot
        screenshot_b64 = base64.b64encode(screenshot).decode("utf-8")

        return StepResponse(
            screenshot=screenshot_b64,
            reward=reward,
            done=done,
        )

    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.get("/screenshot", response_model=ScreenshotResponse)
async def get_screenshot(env_id: int):
    """Get the current screenshot from an environment."""
    state = _get_env(env_id)

    try:
        if state.env.session is None:
            raise HTTPException(status_code=500, detail="No active session")

        screenshot = await state.env.session.screenshot()
        screenshot_b64 = base64.b64encode(screenshot).decode("utf-8")

        return ScreenshotResponse(
            screenshot=screenshot_b64,
            env_id=env_id,
        )

    except Exception as e:
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/shutdown")
async def shutdown(request: ShutdownRequest):
    """Shut down environments.

    If env_id is specified, shuts down that specific environment.
    If env_id is None, shuts down all environments.
    """
    if request.env_id is not None:
        _release_env(request.env_id)
        return {"status": "ok", "message": f"Environment {request.env_id} shut down"}
    else:
        _release_env("all")
        return {"status": "ok", "message": "All environments shut down"}


@app.get("/health", response_model=HealthResponse)
async def health():
    """Health check endpoint."""
    with env_lock:
        return HealthResponse(
            status="ok",
            available_envs=len(available_envs),
            active_envs=len(active_envs),
            max_envs=MAX_ENVS,
        )


# --- CLI Entry Point ---
def main():
    """Run the worker server."""
    parser = argparse.ArgumentParser(description="CUA-Bench Worker Server")
    parser.add_argument("--host", default="0.0.0.0", help="Host to bind to")
    parser.add_argument("--port", type=int, default=8001, help="Port to bind to")
    parser.add_argument("--reload", action="store_true", help="Enable auto-reload")
    args = parser.parse_args()

    import uvicorn

    uvicorn.run(
        "cua_bench.workers.worker_server:app",
        host=args.host,
        port=args.port,
        reload=args.reload,
    )


if __name__ == "__main__":
    main()
