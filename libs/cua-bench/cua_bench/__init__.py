"""cua-bench SDK - A framework for desktop automation tasks with batch processing."""

from .core import Task, make, interact
from .decorators import tasks_config, setup_task, solve_task, evaluate_task
from .environment import Environment
from .desktop import Desktop
from .computers import DesktopSession
from .types import (
    Action,
    ClickAction,
    DragAction,
    KeyAction,
    HotkeyAction,
    TypeAction,
    WaitAction,
    DoneAction,
    ScrollAction,
    RightClickAction,
    DoubleClickAction,
)
from .actions import repr_to_action

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
