"""cua-bench SDK - A framework for desktop automation tasks with batch processing."""

from .actions import repr_to_action
from .computers import DesktopSession
from .core import Task, interact, make
from .decorators import evaluate_task, setup_task, solve_task, tasks_config
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

from .compat import install_computer_alias as _install_computer_alias
from .compat import install_moved_modules as _install_moved_modules

# `import computer` (the retired cua-computer) resolves to cua-bench's
# compatibility surface when that package is not installed; 0.2.x pulled it
# in and harnesses import it directly. CUA_BENCH_COMPUTER_ALIAS=0 turns it off.
_install_computer_alias()
# cua_bench.workers / cua_bench.trainer moved to the cua-bench-rl package.
_install_moved_modules()

# MobileSession placeholder (not yet implemented)
MobileSession = DesktopSession


class _RemovedDesktop:
    """``cb.Desktop``: the simulated desktop renderer, removed in cua-bench 0.3."""

    def __init__(self, *args, **kwargs) -> None:
        raise RuntimeError(
            "cb.Desktop (the simulated Playwright desktop) was removed in cua-bench 0.3: "
            "tasks run in a real sandbox; open HTML with session.launch_window(html=...)"
        )


# Heavy parts (the gym Environment pulls in HF `datasets` for traces) load on
# first use, so `import cua_bench` stays light: harnesses import every task
# module on the host.
_LAZY = {
    "Environment": ".environment",
    "BenchmarkResult": ".runners",
    "TaskResult": ".runners",
    "run_benchmark": ".runners",
    "run_interactive": ".runners",
    "run_single_task": ".runners",
}


def __getattr__(name: str):
    if name in _LAZY:
        import importlib

        value = getattr(importlib.import_module(_LAZY[name], __name__), name)
        globals()[name] = value
        return value
    if name == "Desktop":
        import warnings

        warnings.warn(
            "cb.Desktop was removed in cua-bench 0.3 (the simulated provider is gone)",
            DeprecationWarning,
            stacklevel=2,
        )
        return _RemovedDesktop
    raise AttributeError(f"module 'cua_bench' has no attribute {name!r}")


def __dir__():
    return sorted(set(globals()) | set(_LAZY))


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
