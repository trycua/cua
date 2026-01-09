import re

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


def repr_to_action(action_repr: str) -> Action:
    if not isinstance(action_repr, str):
        raise ValueError("action_repr must be a string")

    action_repr = action_repr.strip()

    patterns = {
        # Mouse click actions
        r"ClickAction\(x=(\d+),\s*y=(\d+)\)": lambda m: ClickAction(x=int(m[0]), y=int(m[1])),
        r"RightClickAction\(x=(\d+),\s*y=(\d+)\)": lambda m: RightClickAction(
            x=int(m[0]), y=int(m[1])
        ),
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

    for pattern, constructor in patterns.items():
        match = re.match(pattern, action_repr)
        if match:
            return constructor(match.groups())
    raise ValueError(f"Unknown action representation: {action_repr}")
