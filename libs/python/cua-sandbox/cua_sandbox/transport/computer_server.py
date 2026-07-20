"""Shared response handling for computer-server HTTP transports."""

from __future__ import annotations

import base64
import json
from typing import Any, Dict


def parse_command_response(text: str) -> Dict[str, Any]:
    """Extract and validate the first JSON data frame from an SSE response."""
    for line in text.splitlines():
        if not line.startswith("data: "):
            continue
        payload = json.loads(line[6:])
        if isinstance(payload, dict) and not payload.get("success", True):
            if "error" in payload:
                raise RuntimeError(f"Remote error: {payload['error']}")
            parts = []
            return_code = payload.get("return_code")
            if return_code is not None:
                parts.append(f"return_code={return_code}")
            stderr = (payload.get("stderr") or "").strip()
            if stderr:
                parts.append(f"stderr={stderr!r}")
            stdout = (payload.get("stdout") or "").strip()
            if stdout and not stderr:
                parts.append(f"stdout={stdout!r}")
            detail = ", ".join(parts) or "no detail"
            raise RuntimeError(f"Remote error: {detail}")
        return payload
    raise RuntimeError(f"No SSE data frame in response: {text[:200]}")


def decode_screenshot_response(payload: Dict[str, Any]) -> bytes:
    """Decode computer-server's accepted screenshot response shapes."""
    encoded = payload.get("image_data", payload.get("base64_image", payload.get("result", "")))
    if isinstance(encoded, dict):
        encoded = encoded.get(
            "image_data", encoded.get("base64_image", encoded.get("base64", ""))
        )
    return base64.b64decode(encoded)


def normalize_screen_size(payload: Dict[str, Any]) -> Dict[str, int]:
    """Normalize nested computer-server screen-size response shapes."""
    data: Any = payload
    if isinstance(data, dict):
        data = data.get("size", data.get("result", data))
    if isinstance(data, dict):
        width = data.get("width") or data.get("screen_width") or data.get("w")
        height = data.get("height") or data.get("screen_height") or data.get("h")
        if width is not None and height is not None:
            return {"width": int(width), "height": int(height)}
    raise KeyError(f"Cannot extract screen size from response: {payload}")
