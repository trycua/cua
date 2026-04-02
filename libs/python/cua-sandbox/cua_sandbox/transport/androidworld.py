"""AndroidWorldTransport — speaks the AndroidWorld FastAPI server API (port 5000).

The AndroidWorld server exposes:
  POST /reset                               → {"status", "message"}
  GET  /screenshot?wait_to_stabilize=bool   → {"pixels": [R,G,B,...]} flat list
  POST /execute_action                      → {"action_type": ..., ...} → {"status", "message"}
  GET  /health                              → {"status"}
  GET  /suite/task_list?max_index=N         → {"task_list": [...]}
  GET  /suite/task_length?task_type=str     → {"length": N}
  GET  /suite/reinitialize?...              → {"status", "message"}
  POST /task/initialize?task_type=&task_idx= → {"status", "message"}
  POST /task/tear_down?task_type=&task_idx=  → {"status", "message"}
  GET  /task/score?task_type=&task_idx=      → {"score": float}
  GET  /task/goal?task_type=&task_idx=       → {"goal": str}
  GET  /task/template?task_type=&task_idx=   → {"template": str}
  POST /close                               → {"status"}
"""

from __future__ import annotations

import io
from typing import Any, Dict, List, Optional

import httpx
from cua_sandbox.transport.base import Transport, convert_screenshot


class AndroidWorldTransport(Transport):
    """Transport for containers/VMs running the AndroidWorld FastAPI server."""

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 60.0,
    ):
        self._base_url = base_url.rstrip("/")
        self._timeout = timeout
        self._client: Optional[httpx.AsyncClient] = None

    async def connect(self) -> None:
        self._client = httpx.AsyncClient(
            base_url=self._base_url,
            timeout=self._timeout,
        )

    async def disconnect(self) -> None:
        if self._client:
            await self._client.aclose()
            self._client = None

    async def send(self, action: str, **params: Any) -> Any:
        assert self._client is not None, "Transport not connected"
        if action == "execute_action":
            resp = await self._client.post("/execute_action", json=params)
            resp.raise_for_status()
            return resp.json()
        if action == "reset":
            resp = await self._client.post(
                "/reset", params={"go_home": params.get("go_home", False)}
            )
            resp.raise_for_status()
            return resp.json()
        raise ValueError(f"Unknown action: {action}")

    async def screenshot(self, format: str = "png", quality: int = 95) -> bytes:
        """Capture screenshot from the AndroidWorld server.

        The server returns pixels as a flat RGB list. This method reshapes it
        into an image and encodes it as PNG (or JPEG if requested).
        """
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/screenshot", params={"wait_to_stabilize": False})
        resp.raise_for_status()
        data = resp.json()
        pixels = data["pixels"]
        # pixels is a flat list from numpy .tolist() — could be HxWx3 nested or flat
        png_bytes = _pixels_to_png(pixels)
        return convert_screenshot(png_bytes, format, quality)

    async def get_screen_size(self) -> Dict[str, int]:
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/screenshot", params={"wait_to_stabilize": False})
        resp.raise_for_status()
        data = resp.json()
        pixels = data["pixels"]
        # pixels is HxWx3 nested list from numpy ndarray.tolist()
        if pixels and isinstance(pixels[0], list):
            height = len(pixels)
            width = len(pixels[0]) if height > 0 else 0
        else:
            # flat list — can't infer dimensions without shape
            # AndroidWorld uses 1080x2400 for Pixel 6 by default
            height, width = 2400, 1080
        return {"width": width, "height": height}

    async def get_environment(self) -> str:
        return "android"

    # ── AndroidWorld-specific methods ────────────────────────────────────────

    async def health(self) -> bool:
        """Return True if the server reports healthy."""
        assert self._client is not None, "Transport not connected"
        try:
            resp = await self._client.get("/health")
            return resp.status_code == 200 and resp.json().get("status") == "success"
        except Exception:
            return False

    async def reset(self, go_home: bool = False) -> Dict[str, Any]:
        """Reset the Android environment."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post("/reset", params={"go_home": go_home})
        resp.raise_for_status()
        return resp.json()

    async def execute_action(self, action_dict: Dict[str, Any]) -> Dict[str, Any]:
        """Execute a JSONAction on the Android environment."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post("/execute_action", json=action_dict)
        resp.raise_for_status()
        return resp.json()

    # ── Suite management ─────────────────────────────────────────────────────

    async def suite_task_list(self, max_index: int = -1) -> List[str]:
        """Return the list of task keys in the current suite."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/suite/task_list", params={"max_index": max_index})
        resp.raise_for_status()
        return resp.json()["task_list"]

    async def suite_task_length(self, task_type: str) -> int:
        """Return the number of param combinations for a given task type."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get("/suite/task_length", params={"task_type": task_type})
        resp.raise_for_status()
        return resp.json()["length"]

    async def reinitialize_suite(
        self,
        task_family: str = "android_world",
        n_task_combinations: int = 2,
        seed: int = 42,
    ) -> Dict[str, Any]:
        """Re-initialize the task suite with new parameters."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get(
            "/suite/reinitialize",
            params={
                "task_family": task_family,
                "n_task_combinations": n_task_combinations,
                "seed": seed,
            },
        )
        resp.raise_for_status()
        return resp.json()

    # ── Task lifecycle ───────────────────────────────────────────────────────

    async def initialize_task(self, task_type: str, task_idx: int = 0) -> Dict[str, Any]:
        """Set up the device and app state for a task."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post(
            "/task/initialize",
            params={"task_type": task_type, "task_idx": task_idx},
        )
        resp.raise_for_status()
        return resp.json()

    async def tear_down_task(self, task_type: str, task_idx: int = 0) -> Dict[str, Any]:
        """Restore app state and clean up after a task."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post(
            "/task/tear_down",
            params={"task_type": task_type, "task_idx": task_idx},
        )
        resp.raise_for_status()
        return resp.json()

    async def get_task_score(self, task_type: str, task_idx: int = 0) -> float:
        """Return the success score (0.0–1.0) for the current task state."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get(
            "/task/score",
            params={"task_type": task_type, "task_idx": task_idx},
        )
        resp.raise_for_status()
        return float(resp.json()["score"])

    async def get_task_goal(self, task_type: str, task_idx: int = 0) -> str:
        """Return the natural-language goal string for a task instance."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get(
            "/task/goal",
            params={"task_type": task_type, "task_idx": task_idx},
        )
        resp.raise_for_status()
        return resp.json()["goal"]

    async def get_task_template(self, task_type: str, task_idx: int = 0) -> str:
        """Return the goal template string for a task type."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.get(
            "/task/template",
            params={"task_type": task_type, "task_idx": task_idx},
        )
        resp.raise_for_status()
        return resp.json()["template"]

    async def close(self) -> None:
        """Close the Android environment on the server."""
        assert self._client is not None, "Transport not connected"
        resp = await self._client.post("/close")
        resp.raise_for_status()


# ── Internal helpers ─────────────────────────────────────────────────────────


def _pixels_to_png(pixels: Any) -> bytes:
    """Convert AndroidWorld pixel data (nested or flat list) to PNG bytes."""
    import numpy as np
    from PIL import Image as PILImage

    arr = np.array(pixels, dtype=np.uint8)
    if arr.ndim == 1:
        # Flat — assume Pixel 6 default resolution (2400 x 1080 x 3)
        total = arr.size
        height = 2400
        width = total // (height * 3)
        arr = arr.reshape(height, width, 3)
    elif arr.ndim == 2:
        # HxW grayscale
        pass
    # arr should now be HxWx3
    img = PILImage.fromarray(arr, mode="RGB")
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return buf.getvalue()
