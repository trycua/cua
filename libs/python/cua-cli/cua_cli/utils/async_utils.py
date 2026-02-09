"""Async utilities for CUA CLI."""

import asyncio
from typing import Any, Coroutine, TypeVar

T = TypeVar("T")


def run_async(coro: Coroutine[Any, Any, T]) -> T:
    """Run an async coroutine synchronously.

    This is the standard pattern for CLI commands that need to call async code.

    Args:
        coro: The coroutine to run

    Returns:
        The result of the coroutine
    """
    return asyncio.run(coro)
