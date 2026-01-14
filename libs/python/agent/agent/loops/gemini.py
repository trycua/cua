"""
Gemini Computer Use agent loop

Maps internal Agent SDK message format to Google's Gemini Computer Use API and back.

Supported models:
- gemini-2.5-computer-use-preview-10-2025 (uses built-in ComputerUse tool)
- gemini-3-flash-preview (and variants) (uses custom function declarations)
- gemini-3-pro-preview (and variants) (uses custom function declarations)

Key features:
- Lazy import of google.genai
- Configure Computer Use tool with excluded browser-specific predefined functions (Gemini 2.5)
- Custom function declarations for computer use actions (Gemini 3 models)
- Convert Gemini function_call parts into internal computer_call actions
- Gemini 3-specific: thinking_level and media_resolution parameters
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


def _sanitize_for_json(obj: Any) -> Any:
    """
    Recursively sanitize an object for JSON serialization.
    Handles bytes fields (like thought_signature in Gemini 3 responses).
    """
    if obj is None:
        return None
    if isinstance(obj, bytes):
        # Convert bytes to base64 string for JSON serialization
        return f"<bytes:{base64.b64encode(obj).decode('ascii')}>"
    if isinstance(obj, (str, int, float, bool)):
        return obj
    if isinstance(obj, dict):
        return {k: _sanitize_for_json(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_sanitize_for_json(item) for item in obj]
    # Handle objects with __dict__ (like Gemini SDK response objects)
    if hasattr(obj, "__dict__"):
        return {k: _sanitize_for_json(v) for k, v in obj.__dict__.items() if not k.startswith("_")}
    # Handle objects with model_dump (Pydantic models)
    if hasattr(obj, "model_dump"):
        return _sanitize_for_json(obj.model_dump())
    # Fallback to string representation
    return str(obj)


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


def _convert_messages_to_gemini_contents(
    messages: List[Dict[str, Any]],
    types: Any,
) -> Tuple[List[Any], Tuple[int, int]]:
    """
    Convert internal message format to Gemini's Content format with full conversation history.

    Similar to how Anthropic loop uses _convert_responses_items_to_completion_messages,
    this converts ALL messages to Gemini's format.

    Gemini requires:
    - role: "user" or "model"
    - parts: list of Part objects (text, image, function_call, function_response)

    Returns:
        Tuple of (list of Content objects, (screen_width, screen_height))
    """
    contents: List[Any] = []
    screen_w, screen_h = 1024, 768  # Default dimensions

    for msg in messages:
        msg_type = msg.get("type")
        role = msg.get("role")

        # User messages
        if role == "user" or (msg_type in (None, "message") and role == "user"):
            parts: List[Any] = []
            content = msg.get("content")

            if isinstance(content, str):
                parts.append(types.Part(text=content))
            elif isinstance(content, list):
                for c in content:
                    if c.get("type") in ("input_text", "text") and c.get("text"):
                        parts.append(types.Part(text=c["text"]))
                    elif c.get("type") == "input_image" and c.get("image_url"):
                        img_bytes, _ = _data_url_to_bytes(c["image_url"])
                        if img_bytes:
                            w, h = _bytes_image_size(img_bytes)
                            screen_w, screen_h = w, h
                            parts.append(
                                types.Part.from_bytes(data=img_bytes, mime_type="image/png")
                            )

            if parts:
                contents.append(types.Content(role="user", parts=parts))

        # Assistant messages
        elif role == "assistant" or (msg_type == "message" and role == "assistant"):
            parts = []
            content = msg.get("content")

            if isinstance(content, str):
                parts.append(types.Part(text=content))
            elif isinstance(content, list):
                for c in content:
                    if c.get("type") in ("output_text", "text") and c.get("text"):
                        parts.append(types.Part(text=c["text"]))

            if parts:
                contents.append(types.Content(role="model", parts=parts))

        # Reasoning (treat as model output)
        elif msg_type == "reasoning":
            summary = msg.get("summary", [])
            for s in summary:
                if s.get("type") == "summary_text" and s.get("text"):
                    contents.append(
                        types.Content(
                            role="model", parts=[types.Part(text=f"[Thinking: {s['text']}]")]
                        )
                    )
                    break

        # Computer call (model action) - represent as text description for context
        elif msg_type == "computer_call":
            action = msg.get("action", {})
            action_type = action.get("type", "unknown")
            action_desc = f"[Action: {action_type}"
            for k, v in action.items():
                if k != "type":
                    action_desc += f", {k}={v}"
            action_desc += "]"
            contents.append(types.Content(role="model", parts=[types.Part(text=action_desc)]))

        # Computer call output (screenshot result) - this is the key part!
        elif msg_type == "computer_call_output":
            out = msg.get("output", {})
            if isinstance(out, dict) and out.get("type") in ("input_image", "computer_screenshot"):
                image_url = out.get("image_url", "")
                if image_url and image_url != "[omitted]":
                    img_bytes, _ = _data_url_to_bytes(image_url)
                    if img_bytes:
                        w, h = _bytes_image_size(img_bytes)
                        screen_w, screen_h = w, h
                        contents.append(
                            types.Content(
                                role="user",
                                parts=[
                                    types.Part.from_bytes(data=img_bytes, mime_type="image/png")
                                ],
                            )
                        )
                else:
                    # Image was omitted (by ImageRetentionCallback)
                    contents.append(
                        types.Content(
                            role="user",
                            parts=[
                                types.Part(
                                    text="[Screenshot taken - image omitted for context limit]"
                                )
                            ],
                        )
                    )

        # Function call (model action)
        elif msg_type == "function_call":
            fn_name = msg.get("name", "unknown")
            fn_args = msg.get("arguments", "{}")
            contents.append(
                types.Content(
                    role="model", parts=[types.Part(text=f"[Function call: {fn_name}({fn_args})]")]
                )
            )

        # Function call output
        elif msg_type == "function_call_output":
            output = msg.get("output", "")
            contents.append(
                types.Content(role="user", parts=[types.Part(text=f"[Function result: {output}]")])
            )

    # Gemini requires alternating user/model turns - merge consecutive same-role contents
    merged: List[Any] = []
    for content in contents:
        if merged and merged[-1].role == content.role:
            # Merge parts into the previous content of same role
            merged[-1] = types.Content(
                role=content.role, parts=list(merged[-1].parts) + list(content.parts)
            )
        else:
            merged.append(content)

    # Gemini requires conversation to start with user
    if merged and merged[0].role == "model":
        merged.insert(0, types.Content(role="user", parts=[types.Part(text="Begin the task.")]))

    # Ensure we have at least one message
    if not merged:
        merged = [
            types.Content(role="user", parts=[types.Part(text="Proceed to the next action.")])
        ]

    return merged, (screen_w, screen_h)


def _denormalize(v: int, size: int) -> int:
    # Gemini returns 0-999 normalized
    try:
        return max(0, min(size - 1, int(round(v / 1000 * size))))
    except Exception:
        return 0


def _is_gemini_3_model(model: str) -> bool:
    """Check if the model is a Gemini 3 model (Flash or Pro Preview)."""
    return "gemini-3" in model.lower() or "gemini-2.0" in model.lower()


def _build_custom_function_declarations(types: Any) -> List[Any]:
    """
    Build custom function declarations for Gemini 3 models.

    These function declarations replicate the built-in ComputerUse tool actions
    that are available in Gemini 2.5 Computer Use Preview, but using the standard
    function calling interface.

    Note: Coordinates use 0-999 normalized range for both x and y.
    """
    return [
        types.FunctionDeclaration(
            name="click_at",
            description="Click at the specified x,y coordinates on the screen. Coordinates are normalized 0-999.",
            parameters={
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate (0-999 normalized)"},
                    "y": {"type": "integer", "description": "Y coordinate (0-999 normalized)"},
                },
                "required": ["x", "y"],
            },
        ),
        types.FunctionDeclaration(
            name="type_text_at",
            description="Type text at the specified x,y coordinates. First clicks at the location, then types the text.",
            parameters={
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate (0-999 normalized)"},
                    "y": {"type": "integer", "description": "Y coordinate (0-999 normalized)"},
                    "text": {"type": "string", "description": "Text to type"},
                    "press_enter": {
                        "type": "boolean",
                        "description": "Whether to press Enter after typing",
                    },
                },
                "required": ["x", "y", "text"],
            },
        ),
        types.FunctionDeclaration(
            name="hover_at",
            description="Move the mouse cursor to the specified x,y coordinates without clicking.",
            parameters={
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate (0-999 normalized)"},
                    "y": {"type": "integer", "description": "Y coordinate (0-999 normalized)"},
                },
                "required": ["x", "y"],
            },
        ),
        types.FunctionDeclaration(
            name="key_combination",
            description="Press a key combination (e.g., 'ctrl+c', 'alt+tab', 'enter').",
            parameters={
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "string",
                        "description": "Key combination to press (e.g., 'ctrl+c', 'enter', 'alt+tab')",
                    },
                },
                "required": ["keys"],
            },
        ),
        types.FunctionDeclaration(
            name="scroll_at",
            description="Scroll at the specified x,y coordinates in a given direction.",
            parameters={
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate (0-999 normalized)"},
                    "y": {"type": "integer", "description": "Y coordinate (0-999 normalized)"},
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down", "left", "right"],
                        "description": "Direction to scroll",
                    },
                    "magnitude": {
                        "type": "integer",
                        "description": "Amount to scroll in pixels (default 800)",
                    },
                },
                "required": ["x", "y", "direction"],
            },
        ),
        types.FunctionDeclaration(
            name="scroll_document",
            description="Scroll the entire document/page in a given direction.",
            parameters={
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down", "left", "right"],
                        "description": "Direction to scroll",
                    },
                },
                "required": ["direction"],
            },
        ),
        types.FunctionDeclaration(
            name="drag_and_drop",
            description="Drag from one coordinate to another.",
            parameters={
                "type": "object",
                "properties": {
                    "x": {
                        "type": "integer",
                        "description": "Starting X coordinate (0-999 normalized)",
                    },
                    "y": {
                        "type": "integer",
                        "description": "Starting Y coordinate (0-999 normalized)",
                    },
                    "destination_x": {
                        "type": "integer",
                        "description": "Destination X coordinate (0-999 normalized)",
                    },
                    "destination_y": {
                        "type": "integer",
                        "description": "Destination Y coordinate (0-999 normalized)",
                    },
                },
                "required": ["x", "y", "destination_x", "destination_y"],
            },
        ),
        types.FunctionDeclaration(
            name="wait_5_seconds",
            description="Wait for 5 seconds before the next action. Use this when waiting for page loads or animations.",
            parameters={
                "type": "object",
                "properties": {},
            },
        ),
        # # Browser-specific functions -> commented out for future support of browser exposed functions
        # types.FunctionDeclaration(
        #     name="navigate",
        #     description="Navigate the browser to a specific URL.",
        #     parameters={
        #         "type": "object",
        #         "properties": {
        #             "url": {"type": "string", "description": "URL to navigate to"},
        #         },
        #         "required": ["url"],
        #     },
        # ),
        # types.FunctionDeclaration(
        #     name="open_web_browser",
        #     description="Open a web browser.",
        #     parameters={
        #         "type": "object",
        #         "properties": {},
        #     },
        # ),
        # types.FunctionDeclaration(
        #     name="search",
        #     description="Perform a web search with the given query.",
        #     parameters={
        #         "type": "object",
        #         "properties": {
        #             "query": {"type": "string", "description": "Search query"},
        #         },
        #         "required": ["query"],
        #     },
        # ),
        # types.FunctionDeclaration(
        #     name="go_back",
        #     description="Go back to the previous page in the browser.",
        #     parameters={
        #         "type": "object",
        #         "properties": {},
        #     },
        # ),
        # types.FunctionDeclaration(
        #     name="go_forward",
        #     description="Go forward to the next page in the browser.",
        #     parameters={
        #         "type": "object",
        #         "properties": {},
        #     },
        # ),
    ]


def _map_gemini_fc_to_computer_call(
    fc: Dict[str, Any],
    screen_w: int,
    screen_h: int,
) -> Optional[Dict[str, Any]]:
    name = fc.get("name")
    args = fc.get("args", {}) or {}

    # Gemini 3 Flash uses "web_agent_api:" prefix for browser functions
    # Strip the prefix to normalize function names
    if name and name.startswith("web_agent_api:"):
        name = name[len("web_agent_api:") :]

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
    # Browser-specific functions - use playwright_exec for browser control
    # (Note: Gemini API does not respect exclusions, so we implement these)
    elif name == "navigate":
        url = args.get("url", "")
        if url:
            action = {"type": "playwright_exec", "command": "visit_url", "params": {"url": url}}
        else:
            return None
    elif name == "open_web_browser":
        # Open browser with blank page or google
        action = {
            "type": "playwright_exec",
            "command": "visit_url",
            "params": {"url": "https://www.google.com"},
        }
    elif name == "search":
        query = args.get("query", "")
        if query:
            action = {
                "type": "playwright_exec",
                "command": "web_search",
                "params": {"query": query},
            }
        else:
            return None
    elif name == "go_back":
        # Browser back via Playwright's native navigation
        action = {"type": "playwright_exec", "command": "go_back", "params": {}}
    elif name == "go_forward":
        # Browser forward via Playwright's native navigation
        action = {"type": "playwright_exec", "command": "go_forward", "params": {}}
    else:
        # Unsupported / unknown function
        print(f"[WARN] Unsupported Gemini function: {name}")
        return None

    return {
        "type": "computer_call",
        "call_id": uuid.uuid4().hex,
        "status": "completed",
        "action": action,
    }


# Supported models:
# - gemini-2.5-computer-use-preview-* : Uses built-in ComputerUse tool
# - gemini-3-flash-preview-* : Uses custom function declarations
# - gemini-3-pro-preview-* : Uses custom function declarations
@register_agent(
    models=r"^(gemini-2\.5-computer-use-preview.*|gemini-3-flash-preview.*|gemini-3-pro-preview.*)$"
)
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
        import os

        # Authentication follows two modes based on environment variables:
        # 1. Google AI Studio: Set GOOGLE_API_KEY
        # 2. Vertex AI: Set GOOGLE_CLOUD_PROJECT, GOOGLE_CLOUD_LOCATION, GOOGLE_GENAI_USE_VERTEXAI=True
        api_key = kwargs.get("api_key", os.getenv("GOOGLE_API_KEY"))

        if api_key:
            client = genai.Client(api_key=api_key)
        else:
            # Vertex AI mode - requires GOOGLE_CLOUD_PROJECT, GOOGLE_CLOUD_LOCATION env vars
            # and Application Default Credentials (ADC)
            client = genai.Client()

        # Extract Gemini 3-specific parameters
        # thinking_level: Use types.ThinkingLevel enum values (e.g., "LOW", "HIGH", "MEDIUM", "MINIMAL")
        # media_resolution: Use types.MediaResolution enum values (e.g., "MEDIA_RESOLUTION_LOW", "MEDIA_RESOLUTION_HIGH")
        thinking_level = kwargs.pop("thinking_level", None)
        media_resolution = kwargs.pop("media_resolution", None)

        # Build thinking_config for Gemini 3 models if specified
        thinking_config = None
        if thinking_level:
            # Accept string values and map to SDK enum
            level_map = {
                "minimal": types.ThinkingLevel.MINIMAL,
                "low": types.ThinkingLevel.LOW,
                "medium": types.ThinkingLevel.MEDIUM,
                "high": types.ThinkingLevel.HIGH,
            }
            # Handle both lowercase strings and SDK enum values
            if isinstance(thinking_level, str) and thinking_level.lower() in level_map:
                thinking_config = types.ThinkingConfig(
                    thinking_level=level_map[thinking_level.lower()]
                )
            else:
                # Assume it's already an SDK enum value
                thinking_config = types.ThinkingConfig(thinking_level=thinking_level)

        # Build media_resolution for Gemini 3 models if specified
        resolved_media_resolution = None
        if media_resolution:
            resolution_map = {
                "low": types.MediaResolution.MEDIA_RESOLUTION_LOW,
                "medium": types.MediaResolution.MEDIA_RESOLUTION_MEDIUM,
                "high": types.MediaResolution.MEDIA_RESOLUTION_HIGH,
            }
            if isinstance(media_resolution, str) and media_resolution.lower() in resolution_map:
                resolved_media_resolution = resolution_map[media_resolution.lower()]
            else:
                # Assume it's already an SDK enum value
                resolved_media_resolution = media_resolution

        # Compose tools config based on model type
        # Gemini 2.5 Computer Use Preview uses built-in ComputerUse tool
        # Gemini 3 Flash/Pro Preview uses custom function declarations
        is_gemini_3 = _is_gemini_3_model(model)

        if is_gemini_3:
            # Use custom function declarations for Gemini 3 models
            custom_functions = _build_custom_function_declarations(types)
            print(f"[DEBUG] Using custom function declarations for Gemini 3 model: {model}")
            print(f"[DEBUG] Number of custom functions: {len(custom_functions)}")

            generate_content_config = types.GenerateContentConfig(
                tools=[
                    types.Tool(function_declarations=custom_functions),
                ],
                thinking_config=thinking_config,
                media_resolution=resolved_media_resolution,
            )
        else:
            excluded = [
                "open_web_browser",
                "search",
                "navigate",
                "go_forward",
                "go_back",
                "scroll_document",
            ]

            # Note: ENVIRONMENT_BROWSER biases model towards browser actions
            # Use ENVIRONMENT_UNSPECIFIED for general desktop tasks
            computer_environment = kwargs.pop("computer_environment", "browser")
            env_map = {
                "browser": types.Environment.ENVIRONMENT_BROWSER,
                "unspecified": types.Environment.ENVIRONMENT_UNSPECIFIED,
            }
            resolved_environment = env_map.get(
                computer_environment.lower(), types.Environment.ENVIRONMENT_BROWSER
            )

            print(f"[DEBUG] Using built-in ComputerUse tool for Gemini 2.5 model: {model}")
            print(f"[DEBUG] Environment: {resolved_environment}")
            print(f"[DEBUG] Excluded functions: {excluded}")

            generate_content_config = types.GenerateContentConfig(
                tools=[
                    types.Tool(
                        computer_use=types.ComputerUse(
                            environment=resolved_environment,
                            excluded_predefined_functions=excluded,
                        )
                    ),
                ],
                thinking_config=thinking_config,
                media_resolution=resolved_media_resolution,
            )

        # Convert full message history to Gemini Contents format
        contents, (screen_w, screen_h) = _convert_messages_to_gemini_contents(messages, types)

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

        # Debug: print raw function calls from response
        try:
            for p in response.candidates[0].content.parts:
                if hasattr(p, "function_call") and p.function_call:
                    print(
                        f"[DEBUG] Raw function_call from model: name={p.function_call.name}, args={dict(p.function_call.args or {})}"
                    )
        except Exception as e:
            print(f"[DEBUG] Error printing function calls: {e}")

        if _on_api_end:
            # Sanitize response to handle bytes fields (e.g., thought_signature in Gemini 3)
            await _on_api_end(
                {
                    "model": api_kwargs["model"],
                    # "contents": api_kwargs["contents"], # Disabled for now
                    "config": api_kwargs["config"],
                },
                _sanitize_for_json(response),
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
            print(f"[DEBUG] Model returned function_call: {fc}")
            item = _map_gemini_fc_to_computer_call(fc, screen_w, screen_h)
            if item is not None:
                output_items.append(item)
            else:
                print(f"[DEBUG] Function '{fc.get('name')}' not mapped (excluded or unsupported)")

        return {"output": output_items, "usage": usage}

    async def predict_click(
        self,
        model: str,
        image_b64: str,
        instruction: str,
        **kwargs,
    ) -> Optional[Tuple[float, float]]:
        """Ask Gemini Cua to output a single click action for the given instruction.

        For Gemini 2.5: Excludes all predefined tools except `click_at` and sends the screenshot.
        For Gemini 3: Uses only the click_at function declaration.
        Returns pixel (x, y) if a click is proposed, else None.
        """
        genai, types = _lazy_import_genai()
        import os

        # Authentication: GOOGLE_API_KEY for AI Studio, or Vertex AI env vars
        api_key = kwargs.get("api_key", os.getenv("GOOGLE_API_KEY"))
        if api_key:
            client = genai.Client(api_key=api_key)
        else:
            client = genai.Client()

        # Build tools config based on model type
        is_gemini_3 = _is_gemini_3_model(model)

        if is_gemini_3:
            # For Gemini 3 models, use only click_at function declaration
            click_function = types.FunctionDeclaration(
                name="click_at",
                description="Click at the specified x,y coordinates on the screen. Coordinates are normalized 0-999.",
                parameters={
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate (0-999 normalized)"},
                        "y": {"type": "integer", "description": "Y coordinate (0-999 normalized)"},
                    },
                    "required": ["x", "y"],
                },
            )
            config = types.GenerateContentConfig(
                tools=[
                    types.Tool(function_declarations=[click_function]),
                ]
            )
        else:
            exclude_all_but_click = [
                "open_web_browser",
                "search",
                "navigate",
                "go_forward",
                "go_back",
                "scroll_document",
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
