"""Backend-specific exposure policy for computer-server ingress surfaces."""

import os
from typing import Any, Dict, Mapping, TypeVar

VNC_UNSUPPORTED_CODE = "unsupported_in_vnc"
VNC_UNSUPPORTED_ERROR = (
    "This operation is unavailable with the VNC backend because VNC only "
    "provides remote screen, pointer, scroll, and keyboard control."
)

# This is deliberately an allowlist. New commands and MCP tools stay hidden in
# VNC mode until they are proven to operate through the remote VNC connection.
VNC_REMOTE_COMMANDS = frozenset(
    {
        "version",
        "mouse_down",
        "mouse_up",
        "left_click",
        "right_click",
        "double_click",
        "move_cursor",
        "drag_to",
        "drag",
        "key_down",
        "key_up",
        "type_text",
        "press_key",
        "hotkey",
        "scroll",
        "scroll_down",
        "scroll_up",
        "scroll_direction",
        "screenshot",
        "get_cursor_position",
        "get_screen_size",
    }
)

VNC_REMOTE_MCP_TOOLS = frozenset(
    {
        "computer_screenshot",
        "computer_get_screen_size",
        "computer_get_cursor_position",
        "computer_click",
        "computer_double_click",
        "computer_move",
        "computer_drag",
        "computer_scroll",
        "computer_mouse_down",
        "computer_mouse_up",
        "computer_type",
        "computer_press_key",
        "computer_hotkey",
        "computer_key_down",
        "computer_key_up",
    }
)


def configured_backend() -> str:
    """Return the effective backend using the same selection rule as the factory."""

    backend = os.environ.get("CUA_BACKEND", "native").strip().lower()
    if backend == "vnc" or os.environ.get("CUA_VNC_HOST"):
        return "vnc"
    return backend


def is_vnc_backend() -> bool:
    return configured_backend() == "vnc"


def vnc_unsupported_result() -> Dict[str, Any]:
    """Return the stable refusal shared by defensive handler fallbacks."""

    return {
        "success": False,
        "code": VNC_UNSUPPORTED_CODE,
        "error": VNC_UNSUPPORTED_ERROR,
    }


RegistryValue = TypeVar("RegistryValue")


def exposed_command_registry(
    registry: Mapping[str, RegistryValue],
) -> Dict[str, RegistryValue]:
    """Return the commands safe to advertise for the configured backend."""

    if not is_vnc_backend():
        return dict(registry)
    return {name: value for name, value in registry.items() if name in VNC_REMOTE_COMMANDS}


class VNCUnavailableHandler:
    """Fail-closed placeholder for capabilities VNC cannot address remotely."""

    def __getattr__(self, _name: str):
        async def refuse(*_args, **_kwargs) -> Dict[str, Any]:
            return vnc_unsupported_result()

        return refuse
