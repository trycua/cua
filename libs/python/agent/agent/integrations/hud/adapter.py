"""HUD Adapter for ComputerAgent integration."""

from __future__ import annotations

from typing import Any, ClassVar

from hud.adapters.common import CLA, Adapter
from hud.adapters.common.types import (
    CLAButton,
    CLAKey,
    ClickAction,
    CustomAction,
    DragAction,
    MoveAction,
    Point,
    PressAction,
    ResponseAction,
    ScreenshotFetch,
    ScrollAction,
    TypeAction,
    WaitAction,
)


class ComputerAgentAdapter(Adapter):
    """Adapter for ComputerAgent to work with HUD."""
    
    KEY_MAP: ClassVar[dict[str, CLAKey]] = {
        "return": "enter",
        "arrowup": "up",
        "arrowdown": "down",
        "arrowleft": "left",
        "arrowright": "right",
        "cmd": "ctrl",
        "super": "win",
        "meta": "win",
    }

    BUTTON_MAP: ClassVar[dict[str, CLAButton]] = {
        "wheel": "middle",
        "middle": "middle",
    }

    def __init__(self) -> None:
        super().__init__()
        # ComputerAgent default dimensions (can be overridden)
        self.agent_width = 1024
        self.agent_height = 768

    def _map_key(self, key: str) -> CLAKey:
        """Map a key to its standardized form."""
        return self.KEY_MAP.get(key.lower(), key.lower())  # type: ignore

    def convert(self, data: Any) -> CLA:
        """Convert a ComputerAgent action to a HUD action."""
        try:
            action_type = data.get("type")

            if action_type == "click":
                x, y = data.get("x", 0), data.get("y", 0)
                button = data.get("button", "left")
                button = self.BUTTON_MAP.get(button, button)
                if button is None:
                    button = "left"
                converted_action = ClickAction(point=Point(x=x, y=y), button=button)

            elif action_type == "double_click":
                x, y = data.get("x", 0), data.get("y", 0)
                converted_action = ClickAction(point=Point(x=x, y=y), button="left", pattern=[100])

            elif action_type == "scroll":
                x, y = int(data.get("x", 0)), int(data.get("y", 0))
                scroll_x = int(data.get("scroll_x", 0))
                scroll_y = int(data.get("scroll_y", 0))
                converted_action = ScrollAction(
                    point=Point(x=x, y=y), scroll=Point(x=scroll_x, y=scroll_y)
                )

            elif action_type == "type":
                text = data.get("text", "")
                converted_action = TypeAction(text=text, enter_after=False)

            elif action_type == "wait":
                ms = data.get("ms", 1000)
                converted_action = WaitAction(time=ms)

            elif action_type == "move":
                x, y = data.get("x", 0), data.get("y", 0)
                converted_action = MoveAction(point=Point(x=x, y=y))

            elif action_type == "keypress":
                keys = data.get("keys", [])
                if isinstance(keys, str):
                    keys = [keys]
                converted_action = PressAction(keys=[self._map_key(k) for k in keys])

            elif action_type == "drag":
                path = data.get("path", [])
                points = [Point(x=p.get("x", 0), y=p.get("y", 0)) for p in path]
                converted_action = DragAction(path=points)

            elif action_type == "screenshot":
                converted_action = ScreenshotFetch()

            elif action_type == "response":
                converted_action = ResponseAction(text=data.get("text", ""))
                
            elif action_type == "custom":
                converted_action = CustomAction(action=data.get("action", ""))
                
            else:
                raise ValueError(f"Unsupported action type: {action_type}")

            # Add reasoning and logs if available
            converted_action.reasoning = data.get("reasoning", "")
            converted_action.logs = data.get("logs", "")

            return converted_action

        except Exception as e:
            raise ValueError(f"Invalid action: {data}. Error: {e!s}") from e
