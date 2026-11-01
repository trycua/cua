"""Git: the synchronous `sandbox.git` (git commands run in the sandbox)."""

from __future__ import annotations

import functools
import inspect

from e2b.sandbox_async.git import Git as AsyncGit
from e2b.sandbox_sync._loop import Loop


class Git:
    """Module for running git operations in the sandbox."""

    def __init__(self, commands):
        self._g = AsyncGit(commands._c)


def _wrap(fn):
    @functools.wraps(fn)
    def method(self, *args, **kwargs):
        return Loop.run(fn(self._g, *args, **kwargs))

    return method


for _name, _fn in inspect.getmembers(AsyncGit, inspect.iscoroutinefunction):
    if not _name.startswith("_"):
        setattr(Git, _name, _wrap(_fn))

__all__ = ["Git"]
