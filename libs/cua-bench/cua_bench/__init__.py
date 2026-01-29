"""cua-bench SDK - A framework for desktop automation tasks with batch processing."""

from .actions import repr_to_action
from .computers import DesktopSession
from .core import Task, interact, make
from .decorators import evaluate_task, setup_task, solve_task, tasks_config
from .desktop import Desktop
from .environment import Environment
from .runners import (
    BenchmarkResult,
    TaskResult,
    run_benchmark,
    run_interactive,
    run_single_task,
)
from .types import (
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

# MobileSession placeholder (not yet implemented)
MobileSession = DesktopSession

__all__ = [
    # Core
    "Task",
    "make",
    "interact",
    "Environment",
    # Decorators
    "tasks_config",
    "setup_task",
    "solve_task",
    "evaluate_task",
    # Session types
    "Desktop",
    "DesktopSession",
    "MobileSession",
    # Action types
    "Action",
    "ClickAction",
    "RightClickAction",
    "DoubleClickAction",
    "MiddleClickAction",
    "DragAction",
    "MoveToAction",
    "ScrollAction",
    "TypeAction",
    "KeyAction",
    "HotkeyAction",
    "WaitAction",
    "DoneAction",
    # Utilities
    "repr_to_action",
    # Runners
    "run_benchmark",
    "run_single_task",
    "run_interactive",
    "BenchmarkResult",
    "TaskResult",
]
