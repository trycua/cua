"""Maps cua SDK errors (`cua.CuaError.*`) to e2b exceptions.

Matched by class name so this module (and the unit tests) never import the
native `cua` package.
"""

from __future__ import annotations

import asyncio
from typing import Optional, Union

from e2b.exceptions import (
    AuthenticationException,
    FileNotFoundException,
    InvalidArgumentException,
    NotEnoughSpaceException,
    NotFoundException,
    SandboxException,
    SandboxNotFoundException,
    TemplateException,
    TimeoutException,
)


def _is_cua_error(e: BaseException) -> bool:
    return any(c.__name__ == "CuaError" for c in type(e).__mro__)


def map_error(
    e: BaseException,
    *,
    file: bool = False,
    template: bool = False,
    process: Union[bool, int] = False,
    user: Optional[str] = None,
) -> BaseException:
    """The e2b exception for `e`. `file`: a filesystem call (NotFound is a
    missing path); `template`: a create (NotFound is an unknown template);
    `process`: a call on a command or PTY by pid (NotFound is that pid, and so
    is a refused stdin: the process has exited, or its stdin was closed or never
    requested); `user`: the user a command was started as (an unknown one is
    e2b's AuthenticationException)."""
    if isinstance(e, (SandboxException, AuthenticationException, asyncio.CancelledError)):
        return e
    if isinstance(e, (asyncio.TimeoutError, TimeoutError)):
        return TimeoutException(str(e) or "Request timed out")
    # Raised by the backend's `sudo` fallbacks.
    if isinstance(e, FileNotFoundError):
        return FileNotFoundException(str(e))
    if isinstance(e, PermissionError):
        return SandboxException(f"permission denied: {e}")
    if type(e).__name__ == "OutputLost":
        return SandboxException(str(e))
    if not _is_cua_error(e):
        return e
    name = type(e).__name__
    msg = str(e)
    if user and "no such user" in msg:
        return AuthenticationException(f"invalid username: '{user}'")
    if process and "(FailedPrecondition)" in msg:
        pid = None if process is True else process
        return NotFoundException(f"process with pid {pid} not found" if pid is not None else msg)
    if name == "NotFound":
        if file:
            return FileNotFoundException(msg)
        if process:
            pid = None if process is True else process
            return NotFoundException(
                f"process with pid {pid} not found" if pid is not None else msg
            )
        if template:
            return TemplateException(msg)
        return SandboxNotFoundException(msg)
    if name == "ImageNotPublished":
        return TemplateException(msg)
    if name == "Timeout":
        return TimeoutException(msg)
    if name in ("InvalidArgument", "InvalidPlacement", "AmbiguousSandbox"):
        return InvalidArgumentException(msg)
    if name == "PermissionDenied" and file:
        return SandboxException(f"permission denied: {msg}")
    if name in ("Unauthenticated", "PermissionDenied"):
        return AuthenticationException(msg)
    if name == "InsufficientDisk":
        return NotEnoughSpaceException(msg)
    # Transport, Closed, DaemonNotRunning, Runtime, Env, Internal, ...: a
    # SandboxException, which callers treat as transient.
    return SandboxException(f"{name}: {msg}")
