"""
Helper functions and decorators for the Computer module.
"""
import logging
import asyncio
from functools import wraps
from typing import Any, Callable, Optional, TypeVar, cast

# Global reference to the default computer instance
_default_computer = None
"""Global variable storing the default computer instance for use with decorators."""

logger = logging.getLogger(__name__)

def set_default_computer(computer):
    """
    Set the default computer instance to be used by the remote decorator.
    
    Args:
        computer: The computer instance to use as default
    """
    global _default_computer
    _default_computer = computer


def sandboxed(venv_name: str = "default", computer: str = "default", max_retries: int = 3):
    """
    Decorator that wraps a function to be executed remotely via computer.venv_exec.
    
    The decorated function will be executed in a virtual environment on the specified
    computer instance. If execution fails, it will retry up to max_retries times with
    a 1-second delay between attempts.
    
    Args:
        venv_name: Name of the virtual environment to execute in
        computer: The computer instance to use, or "default" to use the globally set default
        max_retries: Maximum number of retries for the remote execution
        
    Returns:
        Callable: An async wrapper function that executes the original function remotely
        
    Raises:
        RuntimeError: If no computer instance is available
        Exception: Re-raises the last exception if all retry attempts fail
    """
    def decorator(func):
        @wraps(func)
        async def wrapper(*args, **kwargs):
            # Determine which computer instance to use
            comp = computer if computer != "default" else _default_computer
            
            if comp is None:
                raise RuntimeError("No computer instance available. Either specify a computer instance or call set_default_computer() first.")
            
            for i in range(max_retries):
                try:
                    return await comp.venv_exec(venv_name, func, *args, **kwargs)
                except Exception as e:
                    logger.error(f"Attempt {i+1} failed: {e}")
                    await asyncio.sleep(1)
                    if i == max_retries - 1:
                        raise e
        return wrapper
    return decorator
