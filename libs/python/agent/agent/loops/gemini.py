"""
Gemini 2.5 Computer Use agent loop

Maps internal Agent SDK message format to Google's Gemini Computer Use API and back.

Key features:
- Lazy import of google.genai
- Configure Computer Use tool with excluded browser-specific predefined functions
- Optional custom function declarations hook for computer-call specific functions
- Convert Gemini function_call parts into internal computer_call actions
"""

from __future__ import annotations

import base64
import io
import uuid
from typing import Any, Dict, List, Optional, Tuple

from PIL import Image

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..types import AgentCapability


def _lazy_import_genai():
    """Import google.genai lazily to avoid hard dependency unless used."""
    try:
        from google import genai  # type: ignore
        from google.genai import types  # type: ignore

        return genai, types
    except Exception as e:  # pragma: no cover
        raise RuntimeError(
            "google.genai is required for the Gemini Computer Use loop. Install the Google Gemini SDK."
        ) from e


def _data_url_to_bytes(data_url: str) -> Tuple[bytes, str]:
    """Convert a data URL to raw bytes and mime type."""
    if not data_url.startswith("data:"):
        # Assume it's base64 png payload
        try:
            return base64.b64decode(data_url), "image/png"
        except Exception:
            return b"", "application/octet-stream"
    header, b64 = data_url.split(",", 1)
    mime = "image/png"
    if ";" in header:
        mime = header.split(";")[0].split(":", 1)[1] or "image/png"
    return base64.b64decode(b64), mime


def _bytes_image_size(img_bytes: bytes) -> Tuple[int, int]:
    try:
        img = Image.open(io.BytesIO(img_bytes))
        return img.size
    except Exception:
        return (1024, 768)


def _find_last_user_text(messages: List[Dict[str, Any]]) -> List[str]:
    texts: List[str] = []
    for msg in reversed(messages):
        if msg.get("type") in (None, "message") and msg.get("role") == "user":
            content = msg.get("content")
            if isinstance(content, str):
                return [content]
            elif isinstance(content, list):
                for c in content:
                    if c.get("type") in ("input_text", "output_text") and c.get("text"):
                        texts.append(c["text"])  # newest first
                if texts:
                    return list(reversed(texts))
    return []


def _find_last_screenshot(messages: List[Dict[str, Any]]) -> Optional[bytes]:
    for msg in reversed(messages):
        if msg.get("type") == "computer_call_output":
            out = msg.get("output", {})
            if isinstance(out, dict) and out.get("type") in ("input_image", "computer_screenshot"):
                image_url = out.get("image_url", "")
                if image_url:
                    data, _ = _data_url_to_bytes(image_url)
                    return data
    return None


def _denormalize(v: int, size: int) -> int:
    # Gemini returns 0-999 normalized
    try:
        return max(0, min(size - 1, int(round(v / 1000 * size))))
    except Exception:
        return 0


def _map_gemini_fc_to_computer_call(
    fc: Dict[str, Any],
    screen_w: int,
    screen_h: int,
) -> Optional[Dict[str, Any]]:
    name = fc.get("name")
    args = fc.get("args", {}) or {}

    action: Dict[str, Any] = {}
    if name == "click_at":
        x = _denormalize(int(args.get("x", 0)), screen_w)
        y = _denormalize(int(args.get("y", 0)), screen_h)
        action = {"type": "click", "x": x, "y": y, "button": "left"}
    elif name == "type_text_at":
        x = _denormalize(int(args.get("x", 0)), screen_w)
        y = _denormalize(int(args.get("y", 0)), screen_h)
        text = args.get("text", "")
        if args.get("press_enter") == True:
            text += "\n"
        action = {"type": "type", "x": x, "y": y, "text": text}
    elif name == "hover_at":
        x = _denormalize(int(args.get("x", 0)), screen_w)
        y = _denormalize(int(args.get("y", 0)), screen_h)
        action = {"type": "move", "x": x, "y": y}
    elif name == "key_combination":
        keys = str(args.get("keys", ""))
        action = {"type": "keypress", "keys": keys}
    elif name == "scroll_document":
        direction = args.get("direction", "down")
        magnitude = 800
        dx, dy = 0, 0
        if direction == "down":
            dy = magnitude
        elif direction == "up":
            dy = -magnitude
        elif direction == "right":
            dx = magnitude
        elif direction == "left":
            dx = -magnitude
        action = {
            "type": "scroll",
            "scroll_x": dx,
            "scroll_y": dy,
            "x": int(screen_w / 2),
            "y": int(screen_h / 2),
        }
    elif name == "scroll_at":
        x = _denormalize(int(args.get("x", 500)), screen_w)
        y = _denormalize(int(args.get("y", 500)), screen_h)
        direction = args.get("direction", "down")
        magnitude = int(args.get("magnitude", 800))
        dx, dy = 0, 0
        if direction == "down":
            dy = magnitude
        elif direction == "up":
            dy = -magnitude
        elif direction == "right":
            dx = magnitude
        elif direction == "left":
            dx = -magnitude
        action = {"type": "scroll", "scroll_x": dx, "scroll_y": dy, "x": x, "y": y}
    elif name == "drag_and_drop":
        x = _denormalize(int(args.get("x", 0)), screen_w)
        y = _denormalize(int(args.get("y", 0)), screen_h)
        dx = _denormalize(int(args.get("destination_x", x)), screen_w)
        dy = _denormalize(int(args.get("destination_y", y)), screen_h)
        action = {
            "type": "drag",
            "start_x": x,
            "start_y": y,
            "end_x": dx,
            "end_y": dy,
            "button": "left",
        }
    elif name == "wait_5_seconds":
        action = {"type": "wait"}
    else:
        # Unsupported / excluded browser-specific or custom function; ignore
        return None

    return {
        "type": "computer_call",
        "call_id": uuid.uuid4().hex,
        "status": "completed",
        "action": action,
    }


@register_agent(models=r"^gemini-2\.5-computer-use-preview-10-2025$")
class GeminiComputerUseConfig(AsyncAgentConfig):
    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler=None,
        use_prompt_caching: Optional[bool] = False,
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **kwargs,
    ) -> Dict[str, Any]:
        genai, types = _lazy_import_genai()

        client = genai.Client()

        # Build excluded predefined functions for browser-specific behavior
        excluded = [
            "open_web_browser",
            "search",
            "navigate",
            "go_forward",
            "go_back",
            "scroll_document",
        ]
        # Optional custom functions: can be extended by host code via `tools` parameter later if desired
        CUSTOM_FUNCTION_DECLARATIONS: List[Any] = []

        # Compose tools config
        generate_content_config = types.GenerateContentConfig(
            tools=[
                types.Tool(
                    computer_use=types.ComputerUse(
                        environment=types.Environment.ENVIRONMENT_BROWSER,
                        excluded_predefined_functions=excluded,
                    )
                ),
                # types.Tool(function_declarations=CUSTOM_FUNCTION_DECLARATIONS),  # enable when custom functions needed
            ]
        )

        # Prepare contents: last user text + latest screenshot
        user_texts = _find_last_user_text(messages)
        screenshot_bytes = _find_last_screenshot(messages)

        parts: List[Any] = []
        for t in user_texts:
            parts.append(types.Part(text=t))

        screen_w, screen_h = 1024, 768
        if screenshot_bytes:
            screen_w, screen_h = _bytes_image_size(screenshot_bytes)
            parts.append(types.Part.from_bytes(data=screenshot_bytes, mime_type="image/png"))

        # If we don't have any content, at least pass an empty user part to prompt reasoning
        if not parts:
            parts = [types.Part(text="Proceed to the next action.")]

        contents = [types.Content(role="user", parts=parts)]

        api_kwargs = {
            "model": model,
            "contents": contents,
            "config": generate_content_config,
        }

        if _on_api_start:
            await _on_api_start(
                {
                    "model": api_kwargs["model"],
                    # "contents": api_kwargs["contents"], # Disabled for now
                    "config": api_kwargs["config"],
                }
            )

        response = client.models.generate_content(**api_kwargs)

        if _on_api_end:
            await _on_api_end(
                {
                    "model": api_kwargs["model"],
                    # "contents": api_kwargs["contents"], # Disabled for now
                    "config": api_kwargs["config"],
                },
                response,
            )

        # Usage (Gemini SDK may not always provide token usage; populate when available)
        usage: Dict[str, Any] = {}
        try:
            # Some SDKs expose response.usage; if available, copy
            if getattr(response, "usage_metadata", None):
                md = response.usage_metadata
                usage = {
                    "prompt_tokens": getattr(md, "prompt_token_count", None) or 0,
                    "completion_tokens": getattr(md, "candidates_token_count", None) or 0,
                    "total_tokens": getattr(md, "total_token_count", None) or 0,
                }
        except Exception:
            pass

        if _on_usage and usage:
            await _on_usage(usage)

        # Parse output into internal items
        output_items: List[Dict[str, Any]] = []

        candidate = response.candidates[0]
        # Text parts from the model (assistant message)
        text_parts: List[str] = []
        function_calls: List[Dict[str, Any]] = []
        for p in candidate.content.parts:
            if getattr(p, "text", None):
                text_parts.append(p.text)
            if getattr(p, "function_call", None):
                # p.function_call has name and args
                fc = {
                    "name": getattr(p.function_call, "name", None),
                    "args": dict(getattr(p.function_call, "args", {}) or {}),
                }
                function_calls.append(fc)

        if text_parts:
            output_items.append(
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "\n".join(text_parts)}],
                }
            )

        # Map function calls to internal computer_call actions
        for fc in function_calls:
            item = _map_gemini_fc_to_computer_call(fc, screen_w, screen_h)
            if item is not None:
                output_items.append(item)

        return {"output": output_items, "usage": usage}

    async def predict_click(
        self,
        model: str,
        image_b64: str,
        instruction: str,
        **kwargs,
    ) -> Optional[Tuple[float, float]]:
        """Ask Gemini CUA to output a single click action for the given instruction.

        Excludes all predefined tools except `click_at` and sends the screenshot.
        Returns pixel (x, y) if a click is proposed, else None.
        """
        genai, types = _lazy_import_genai()

        client = genai.Client()

        # Exclude all but click_at
        exclude_all_but_click = [
            "open_web_browser",
            "wait_5_seconds",
            "go_back",
            "go_forward",
            "search",
            "navigate",
            "hover_at",
            "type_text_at",
            "key_combination",
            "scroll_document",
            "scroll_at",
            "drag_and_drop",
        ]

        config = types.GenerateContentConfig(
            tools=[
                types.Tool(
                    computer_use=types.ComputerUse(
                        environment=types.Environment.ENVIRONMENT_BROWSER,
                        excluded_predefined_functions=exclude_all_but_click,
                    )
                )
            ]
        )

        # Prepare prompt parts
        try:
            img_bytes = base64.b64decode(image_b64)
        except Exception:
            img_bytes = b""

        w, h = _bytes_image_size(img_bytes) if img_bytes else (1024, 768)

        parts: List[Any] = [types.Part(text=f"Click {instruction}.")]
        if img_bytes:
            parts.append(types.Part.from_bytes(data=img_bytes, mime_type="image/png"))

        contents = [types.Content(role="user", parts=parts)]

        response = client.models.generate_content(
            model=model,
            contents=contents,
            config=config,
        )

        # Parse first click_at
        try:
            candidate = response.candidates[0]
            for p in candidate.content.parts:
                fc = getattr(p, "function_call", None)
                if fc and getattr(fc, "name", None) == "click_at":
                    args = dict(getattr(fc, "args", {}) or {})
                    x = _denormalize(int(args.get("x", 0)), w)
                    y = _denormalize(int(args.get("y", 0)), h)
                    return float(x), float(y)
        except Exception:
            return None

        return None

    def get_capabilities(self) -> List[AgentCapability]:
        return ["click", "step"]
