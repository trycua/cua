"""
Helper functions and decorators for the Computer module.
"""

import asyncio
import logging
from functools import wraps
from typing import Any, Awaitable, Callable, Optional, TypeVar

try:
    # Python 3.12+ has ParamSpec in typing
    from typing import ParamSpec
except ImportError:  # pragma: no cover
    # Fallback for environments without ParamSpec in typing
    from typing_extensions import ParamSpec  # type: ignore

P = ParamSpec("P")
R = TypeVar("R")

# Global reference to the default computer instance
_default_computer = None

logger = logging.getLogger(__name__)


def set_default_computer(computer: Any) -> None:
    """
    Set the default computer instance to be used by the remote decorator.

    Args:
        computer: The computer instance to use as default
    """
    global _default_computer
    _default_computer = computer


def sandboxed(
    venv_name: str = "default",
    computer: str = "default",
    max_retries: int = 3,
) -> Callable[[Callable[P, R]], Callable[P, Awaitable[R]]]:
    """
    Decorator that wraps a function to be executed remotely via computer.venv_exec

    Args:
        venv_name: Name of the virtual environment to execute in
        computer: The computer instance to use, or "default" to use the globally set default
        max_retries: Maximum number of retries for the remote execution
    """

    def decorator(func: Callable[P, R]) -> Callable[P, Awaitable[R]]:
        @wraps(func)
        async def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
            # Determine which computer instance to use
            comp = computer if computer != "default" else _default_computer

            if comp is None:
                raise RuntimeError(
                    "No computer instance available. Either specify a computer instance or call set_default_computer() first."
                )

            for i in range(max_retries):
                try:
                    return await comp.venv_exec(venv_name, func, *args, **kwargs)
                except Exception as e:
                    logger.error(f"Attempt {i+1} failed: {e}")
                    await asyncio.sleep(1)
                    if i == max_retries - 1:
                        raise e

            # Should be unreachable because we either returned or raised
            raise RuntimeError("sandboxed wrapper reached unreachable code path")

        return wrapper

    return decorator
