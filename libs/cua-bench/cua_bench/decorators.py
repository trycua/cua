"""Decorators for defining cua-bench environments."""

from functools import wraps
from typing import Callable, Optional

# Global registry for environment functions
_env_registry = {}


def _get_env_registry(env_path: str) -> dict:
    """Get or create registry for an environment."""
    if env_path not in _env_registry:
        _env_registry[env_path] = {
            "tasks_config": None,
            "setup_task": None,
            "solve_task": None,
            "evaluate_task": None,
        }
    return _env_registry[env_path]


def tasks_config(_arg: Optional[Callable] = None, /, *args, **kwargs) -> Callable:
    """Marks the function that lists the task's variants.

    The function takes no arguments and returns a list of ``cb.Task``; the
    CLI indexes them from 0 (``--variant-id``). Use as ``@cb.tasks_config``
    or with a split: ``@cb.tasks_config("train")`` (default ``train``).
    """
    # Two modes: bare (@cb.tasks_config) or parameterized (@cb.tasks_config("train") / split="...")
    if callable(_arg):
        # Bare usage
        split_val = kwargs.get("split", "train")
        func = _arg

        def decorator(func_inner: Callable) -> Callable:
            @wraps(func_inner)
            def wrapper(*w_args, **w_kwargs):
                return func_inner(*w_args, **w_kwargs)

            wrapper._td_type = "tasks_config"
            wrapper._td_split = split_val
            return wrapper

        return decorator(func)
    # Parameterized usage
    split: str = "train"
    if _arg is not None:
        if isinstance(_arg, str):
            split = _arg
        else:
            raise TypeError("@cb.tasks_config first argument must be a 'split' string if provided")
    if "split" in kwargs:
        split = kwargs["split"]

    def decorator(func: Callable) -> Callable:
        @wraps(func)
        def wrapper(*args, **kwargs):
            return func(*args, **kwargs)

        wrapper._td_type = "tasks_config"
        wrapper._td_split = split
        return wrapper

    return decorator


def setup_task(_arg: Optional[Callable] = None, /, *args, **kwargs) -> Callable:
    """Marks the function that prepares a variant's sandbox.

    ``async def setup(task_cfg: cb.Task, session: cb.DesktopSession)`` runs
    after the sandbox starts and before the agent or the oracle. Use as
    ``@cb.setup_task`` or with a split: ``@cb.setup_task("train")``.
    """
    # Two modes: bare (@cb.setup_task) or parameterized (@cb.setup_task("train") / split="...")
    if callable(_arg):
        split_val = kwargs.get("split", "train")
        func = _arg

        def decorator(func_inner: Callable) -> Callable:
            @wraps(func_inner)
            def wrapper(*w_args, **w_kwargs):
                return func_inner(*w_args, **w_kwargs)

            wrapper._td_type = "setup_task"
            wrapper._td_split = split_val
            return wrapper

        return decorator(func)
    split: str = "train"
    if _arg is not None:
        if isinstance(_arg, str):
            split = _arg
        else:
            raise TypeError("@cb.setup_task first argument must be a 'split' string if provided")
    if "split" in kwargs:
        split = kwargs["split"]

    def decorator(func: Callable) -> Callable:
        @wraps(func)
        def wrapper(*args, **kwargs):
            return func(*args, **kwargs)

        wrapper._td_type = "setup_task"
        wrapper._td_split = split
        return wrapper

    return decorator


def solve_task(_arg: Optional[Callable] = None, /, *args, **kwargs) -> Callable:
    """Marks the oracle: the function that solves a variant (optional).

    ``async def solve(task_cfg: cb.Task, session: cb.DesktopSession)`` runs
    instead of an agent with ``--oracle`` or when no agent is given. Use as
    ``@cb.solve_task`` or with a split: ``@cb.solve_task("train")``.
    """
    if callable(_arg):
        split_val = kwargs.get("split", "train")
        func = _arg

        def decorator(func_inner: Callable) -> Callable:
            @wraps(func_inner)
            def wrapper(*w_args, **w_kwargs):
                return func_inner(*w_args, **w_kwargs)

            wrapper._td_type = "solve_task"
            wrapper._td_split = split_val
            return wrapper

        return decorator(func)
    split: str = "train"
    if _arg is not None:
        if isinstance(_arg, str):
            split = _arg
        else:
            raise TypeError("@cb.solve_task first argument must be a 'split' string if provided")
    if "split" in kwargs:
        split = kwargs["split"]

    def decorator(func: Callable) -> Callable:
        @wraps(func)
        def wrapper(*args, **kwargs):
            return func(*args, **kwargs)

        wrapper._td_type = "solve_task"
        wrapper._td_split = split
        return wrapper

    return decorator


def evaluate_task(_arg: Optional[Callable] = None, /, *args, **kwargs) -> Callable:
    """Marks the function that scores a variant.

    ``async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession)``
    returns the evaluation, commonly ``list[float]``; the reward is the mean
    of its numbers. Use as ``@cb.evaluate_task`` or with a split:
    ``@cb.evaluate_task("train")``.
    """
    if callable(_arg):
        split_val = kwargs.get("split", "train")
        func = _arg

        def decorator(func_inner: Callable) -> Callable:
            @wraps(func_inner)
            def wrapper(*w_args, **w_kwargs):
                return func_inner(*w_args, **w_kwargs)

            wrapper._td_type = "evaluate_task"
            wrapper._td_split = split_val
            return wrapper

        return decorator(func)
    split: str = "train"
    if _arg is not None:
        if isinstance(_arg, str):
            split = _arg
        else:
            raise TypeError("@cb.evaluate_task first argument must be a 'split' string if provided")
    if "split" in kwargs:
        split = kwargs["split"]

    def decorator(func: Callable) -> Callable:
        @wraps(func)
        def wrapper(*args, **kwargs):
            return func(*args, **kwargs)

        wrapper._td_type = "evaluate_task"
        wrapper._td_split = split
        return wrapper

    return decorator
