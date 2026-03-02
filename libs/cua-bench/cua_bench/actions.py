import re
from typing import Any, Dict, Union

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

# Patterns for repr format: ClickAction(x=100, y=200)
_REPR_PATTERNS = {
    # Mouse click actions
    r"ClickAction\(x=(\d+),\s*y=(\d+)\)": lambda m: ClickAction(x=int(m[0]), y=int(m[1])),
    r"RightClickAction\(x=(\d+),\s*y=(\d+)\)": lambda m: RightClickAction(x=int(m[0]), y=int(m[1])),
    r"DoubleClickAction\(x=(\d+),\s*y=(\d+)\)": lambda m: DoubleClickAction(
        x=int(m[0]), y=int(m[1])
    ),
    r"MiddleClickAction\(x=(\d+),\s*y=(\d+)\)": lambda m: MiddleClickAction(
        x=int(m[0]), y=int(m[1])
    ),
    # Drag and move actions
    r"DragAction\(from_x=(\d+),\s*from_y=(\d+),\s*to_x=(\d+),\s*to_y=(\d+),\s*duration=([0-9.]+)\)": lambda m: DragAction(
        from_x=int(m[0]), from_y=int(m[1]), to_x=int(m[2]), to_y=int(m[3]), duration=float(m[4])
    ),
    r"DragAction\(from_x=(\d+),\s*from_y=(\d+),\s*to_x=(\d+),\s*to_y=(\d+)\)": lambda m: DragAction(
        from_x=int(m[0]), from_y=int(m[1]), to_x=int(m[2]), to_y=int(m[3])
    ),
    r"MoveToAction\(x=(\d+),\s*y=(\d+),\s*duration=([0-9.]+)\)": lambda m: MoveToAction(
        x=int(m[0]), y=int(m[1]), duration=float(m[2])
    ),
    r"MoveToAction\(x=(\d+),\s*y=(\d+)\)": lambda m: MoveToAction(x=int(m[0]), y=int(m[1])),
    # Scroll actions
    r"ScrollAction\(direction=['\"](\w+)['\"]\,\s*amount=(\d+)\)": lambda m: ScrollAction(
        direction=m[0], amount=int(m[1])
    ),
    r"ScrollAction\(amount=(\d+),\s*direction=['\"](\w+)['\"]\)": lambda m: ScrollAction(
        direction=m[1], amount=int(m[0])
    ),
    r"ScrollAction\(direction=['\"](\w+)['\"]\)": lambda m: ScrollAction(direction=m[0]),
    r"ScrollAction\(amount=(\d+)\)": lambda m: ScrollAction(amount=int(m[0])),
    r"ScrollAction\(\)": lambda m: ScrollAction(),
    # Keyboard actions
    r"KeyAction\(key=['\"]([^'\"]+)['\"]\)": lambda m: KeyAction(key=m[0]),
    r"TypeAction\(text=['\"]([^'\"]*)['\"].*?\)": lambda m: TypeAction(text=m[0]),
    r"HotkeyAction\(keys=\[([^\]]+)\].*?\)": lambda m: HotkeyAction(
        keys=[key.strip().strip("'\"") for key in m[0].split(",")]
    ),
    # Control actions
    r"WaitAction\(seconds=([0-9.]+)\)": lambda m: WaitAction(seconds=float(m[0])),
    r"WaitAction\(\)": lambda m: WaitAction(),
    r"DoneAction\(\)": lambda m: DoneAction(),
}

# Patterns for snake_case format: click(0.5, 0.5)
_SNAKE_CASE_PATTERNS = {
    # Mouse click actions - support both int and float coordinates
    r"click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)": lambda m: ClickAction(
        x=_parse_coord(m[0]), y=_parse_coord(m[1])
    ),
    r"right_click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)": lambda m: RightClickAction(
        x=_parse_coord(m[0]), y=_parse_coord(m[1])
    ),
    r"double_click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)": lambda m: DoubleClickAction(
        x=_parse_coord(m[0]), y=_parse_coord(m[1])
    ),
    r"middle_click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)": lambda m: MiddleClickAction(
        x=_parse_coord(m[0]), y=_parse_coord(m[1])
    ),
    # Drag action
    r"drag\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*\)": lambda m: DragAction(
        from_x=_parse_coord(m[0]),
        from_y=_parse_coord(m[1]),
        to_x=_parse_coord(m[2]),
        to_y=_parse_coord(m[3]),
    ),
    # Move action
    r"move_to\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)": lambda m: MoveToAction(
        x=_parse_coord(m[0]), y=_parse_coord(m[1])
    ),
    # Scroll action
    r"scroll\s*\(\s*(\w+)\s*,\s*(\d+)\s*\)": lambda m: ScrollAction(
        direction=m[0], amount=int(m[1])
    ),
    r"scroll\s*\(\s*(\w+)\s*\)": lambda m: ScrollAction(direction=m[0]),
    # Keyboard actions
    r"key\s*\(\s*(\w+)\s*\)": lambda m: KeyAction(key=m[0]),
    r'type\s*\(\s*["\'](.*)["\']\s*\)': lambda m: TypeAction(text=m[0]),
    r"hotkey\s*\(\s*([\w+]+)\s*\)": lambda m: HotkeyAction(keys=m[0].split("+")),
    # Control actions
    r"wait\s*\(\s*([\d.]+)\s*\)": lambda m: WaitAction(seconds=float(m[0])),
    r"wait\s*\(\s*\)": lambda m: WaitAction(),
    r"done\s*\(\s*\)": lambda m: DoneAction(),
}


def _parse_coord(val: str) -> Union[int, float]:
    """Parse a coordinate value, returning int if whole number, float otherwise."""
    f = float(val)
    if f == int(f):
        return int(f)
    return f


def repr_to_action(action_repr: str) -> Action:
    """Parse an action from repr format string.

    Args:
        action_repr: Action string in repr format, e.g., "ClickAction(x=100, y=200)"

    Returns:
        Parsed Action object

    Raises:
        ValueError: If the action string cannot be parsed
    """
    if not isinstance(action_repr, str):
        raise ValueError("action_repr must be a string")

    action_repr = action_repr.strip()

    for pattern, constructor in _REPR_PATTERNS.items():
        match = re.match(pattern, action_repr)
        if match:
            return constructor(match.groups())
    raise ValueError(f"Unknown action representation: {action_repr}")


def snake_case_to_action(action_str: str) -> Action:
    """Parse an action from snake_case format string.

    Args:
        action_str: Action string in snake_case format, e.g., "click(0.5, 0.5)"

    Returns:
        Parsed Action object

    Raises:
        ValueError: If the action string cannot be parsed
    """
    if not isinstance(action_str, str):
        raise ValueError("action_str must be a string")

    action_str = action_str.strip()

    for pattern, constructor in _SNAKE_CASE_PATTERNS.items():
        match = re.match(pattern, action_str)
        if match:
            return constructor(match.groups())
    raise ValueError(f"Unknown action string: {action_str}")


def parse_action_string(action_str: str) -> Action:
    """Parse an action from either repr or snake_case format.

    This is the unified entry point for parsing action strings.
    It automatically detects the format and delegates to the appropriate parser.

    Args:
        action_str: Action string in either format:
            - Repr format: "ClickAction(x=100, y=200)"
            - Snake_case format: "click(0.5, 0.5)"

    Returns:
        Parsed Action object

    Raises:
        ValueError: If the action string cannot be parsed in either format
    """
    if not isinstance(action_str, str):
        raise ValueError("action_str must be a string")

    action_str = action_str.strip()

    # Try repr format first (check if it starts with an Action class name)
    if re.match(r"[A-Z]\w+Action\(", action_str):
        try:
            return repr_to_action(action_str)
        except ValueError:
            pass

    # Try snake_case format
    try:
        return snake_case_to_action(action_str)
    except ValueError:
        pass

    raise ValueError(f"Could not parse action string: {action_str}")


def action_to_dict(action: Action) -> Dict[str, Any]:
    """Convert an Action object to a dictionary.

    Args:
        action: Action object to convert

    Returns:
        Dictionary representation of the action with 'type' key
    """
    action_type = type(action).__name__

    if isinstance(action, ClickAction):
        return {"type": action_type, "x": action.x, "y": action.y}
    elif isinstance(action, RightClickAction):
        return {"type": action_type, "x": action.x, "y": action.y}
    elif isinstance(action, DoubleClickAction):
        return {"type": action_type, "x": action.x, "y": action.y}
    elif isinstance(action, MiddleClickAction):
        return {"type": action_type, "x": action.x, "y": action.y}
    elif isinstance(action, MoveToAction):
        return {"type": action_type, "x": action.x, "y": action.y, "duration": action.duration}
    elif isinstance(action, DragAction):
        return {
            "type": action_type,
            "from_x": action.from_x,
            "from_y": action.from_y,
            "to_x": action.to_x,
            "to_y": action.to_y,
            "duration": action.duration,
        }
    elif isinstance(action, ScrollAction):
        return {"type": action_type, "direction": action.direction, "amount": action.amount}
    elif isinstance(action, KeyAction):
        return {"type": action_type, "key": action.key}
    elif isinstance(action, TypeAction):
        return {"type": action_type, "text": action.text}
    elif isinstance(action, HotkeyAction):
        return {"type": action_type, "keys": action.keys}
    elif isinstance(action, WaitAction):
        return {"type": action_type, "seconds": action.seconds}
    elif isinstance(action, DoneAction):
        return {"type": action_type}
    else:
        raise ValueError(f"Unknown action type: {type(action)}")


def dict_to_action(action_dict: Dict[str, Any]) -> Action:
    """Convert a dictionary to an Action object.

    Args:
        action_dict: Dictionary with 'type' key and action parameters

    Returns:
        Action object

    Raises:
        ValueError: If the action type is unknown
    """
    action_type = action_dict.get("type") or action_dict.get("action_type", "")
    action_type_normalized = action_type.lower().replace("_", "").replace("action", "")

    if action_type_normalized in ("click", "clickaction"):
        return ClickAction(
            x=_parse_coord(str(action_dict.get("x", 0))),
            y=_parse_coord(str(action_dict.get("y", 0))),
        )
    elif action_type_normalized in ("rightclick", "rightclickaction"):
        return RightClickAction(
            x=_parse_coord(str(action_dict.get("x", 0))),
            y=_parse_coord(str(action_dict.get("y", 0))),
        )
    elif action_type_normalized in ("doubleclick", "doubleclickaction"):
        return DoubleClickAction(
            x=_parse_coord(str(action_dict.get("x", 0))),
            y=_parse_coord(str(action_dict.get("y", 0))),
        )
    elif action_type_normalized in ("middleclick", "middleclickaction"):
        return MiddleClickAction(
            x=_parse_coord(str(action_dict.get("x", 0))),
            y=_parse_coord(str(action_dict.get("y", 0))),
        )
    elif action_type_normalized in ("moveto", "movetoaction"):
        return MoveToAction(
            x=_parse_coord(str(action_dict.get("x", 0))),
            y=_parse_coord(str(action_dict.get("y", 0))),
            duration=float(action_dict.get("duration", 0.5)),
        )
    elif action_type_normalized in ("drag", "dragaction"):
        return DragAction(
            from_x=_parse_coord(str(action_dict.get("from_x", 0))),
            from_y=_parse_coord(str(action_dict.get("from_y", 0))),
            to_x=_parse_coord(str(action_dict.get("to_x", 0))),
            to_y=_parse_coord(str(action_dict.get("to_y", 0))),
            duration=float(action_dict.get("duration", 1.0)),
        )
    elif action_type_normalized in ("scroll", "scrollaction"):
        return ScrollAction(
            direction=action_dict.get("direction", "up"),
            amount=int(action_dict.get("amount", 100)),
        )
    elif action_type_normalized in ("key", "keyaction"):
        return KeyAction(key=action_dict.get("key", ""))
    elif action_type_normalized in ("type", "typeaction"):
        return TypeAction(text=action_dict.get("text", ""))
    elif action_type_normalized in ("hotkey", "hotkeyaction"):
        return HotkeyAction(keys=action_dict.get("keys", []))
    elif action_type_normalized in ("wait", "waitaction"):
        return WaitAction(seconds=float(action_dict.get("seconds", 1.0)))
    elif action_type_normalized in ("done", "doneaction"):
        return DoneAction()
    else:
        raise ValueError(f"Unknown action type: {action_type}")
