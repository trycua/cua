"""Readable errors and opt-in progress for the generated native binding.

- Every ``CuaError`` has ``.code`` (a stable id such as ``not_found``) and
  ``.hint`` (the next step, one sentence); ``str(e)`` stays the raw text.
  ``.report`` has both plus the raw ``detail``.
- A failing call raises the error ``from None`` with the generated binding's
  frames cut: the traceback ends at the line of your code that made the call,
  not 17 frames of plumbing.
- ``Sandboxes.create(options, on_progress=fn)`` calls ``fn`` with each step of
  the create (``phase``, ``detail``, ``fraction``, ``bytes_done`` ...);
  ``cua.enable_output()`` prints the same steps the ``cua`` CLI does. The SDK
  is silent otherwise.
"""

from __future__ import annotations

import functools
import inspect
from typing import Any, Callable

from . import _native


def _report(error: BaseException) -> Any:
    # A subclass (PlanLimitError) reports as the variant it extends.
    variant = next(
        (c.__name__ for c in type(error).__mro__ if c.__qualname__.startswith("CuaError.")),
        type(error).__name__,
    )
    return _native.error_report(variant, str(error))


def install_error_properties() -> None:
    """Adds ``code``, ``hint`` and ``report`` to every ``CuaError`` variant."""
    base = _native.CuaError
    base.report = property(_report)  # type: ignore[attr-defined]
    base.code = property(lambda self: _report(self).code)  # type: ignore[attr-defined]
    base.hint = property(lambda self: _report(self).hint)  # type: ignore[attr-defined]


def _quiet(fn: Callable[..., Any]) -> Callable[..., Any]:
    """``fn`` raising ``CuaError`` without the binding's frames."""
    if inspect.iscoroutinefunction(fn):

        @functools.wraps(fn)
        async def wrapper(*args: Any, **kwargs: Any) -> Any:
            try:
                return await fn(*args, **kwargs)
            except _native.CuaError as e:
                raise e.with_traceback(None) from None

    else:

        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            try:
                return fn(*args, **kwargs)
            except _native.CuaError as e:
                raise e.with_traceback(None) from None

    wrapper.__cua_quiet__ = True  # type: ignore[attr-defined]
    return wrapper


def install_short_tracebacks() -> None:
    """Wraps the public methods of every generated object class."""
    for obj in list(vars(_native).values()):
        if not (inspect.isclass(obj) and "_handle" in getattr(obj, "__annotations__", {})):
            continue
        for name, member in list(vars(obj).items()):
            if name.startswith("_") or getattr(member, "__cua_quiet__", False):
                continue
            if inspect.iscoroutinefunction(member) or inspect.isfunction(member):
                setattr(obj, name, _quiet(member))


class _ProgressListener(_native.SpaceCreateListener):
    def __init__(self, callback: Callable[[Any], None]) -> None:
        self._callback = callback

    def on_progress(self, progress: Any) -> None:
        try:
            self._callback(progress)
        except Exception:  # noqa: BLE001 - a progress callback never fails a create
            pass


def install_on_progress() -> None:
    """``Sandboxes.create(options, on_progress=fn)``."""
    plain = _native.Sandboxes.create

    @functools.wraps(plain)
    async def create(self: Any, options: Any, *, on_progress: Callable[[Any], None] | None = None) -> Any:
        if on_progress is None:
            return await plain(self, options)
        return await self.create_with_progress(options, _ProgressListener(on_progress))

    _native.Sandboxes.create = create  # type: ignore[method-assign]
