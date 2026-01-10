"""cua-bench SDK - A framework for desktop automation tasks with batch processing."""

from .actions import repr_to_action
from .computers import DesktopSession
from .core import Task, interact, make
from .decorators import evaluate_task, setup_task, solve_task, tasks_config
from .desktop import Desktop
from .environment import Environment
from .types import (
    Action,
    ClickAction,
    DoneAction,
    DoubleClickAction,
    DragAction,
    HotkeyAction,
    KeyAction,
    RightClickAction,
    ScrollAction,
    TypeAction,
    WaitAction,
)

# MobileSession placeholder (not yet implemented)
MobileSession = DesktopSession

__all__ = [
    "Task",
    "make",
    "interact",
    "tasks_config",
    "setup_task",
    "solve_task",
    "evaluate_task",
    "Environment",
    "Desktop",
    "DesktopSession",
    "MobileSession",
    "Action",
    "ClickAction",
    "DragAction",
    "KeyAction",
    "HotkeyAction",
    "TypeAction",
    "WaitAction",
    "DoneAction",
    "ScrollAction",
    "RightClickAction",
    "DoubleClickAction",
    "repr_to_action",
]
