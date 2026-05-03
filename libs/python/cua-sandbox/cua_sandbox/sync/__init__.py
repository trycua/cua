"""Synchronous API wrappers for scripts and notebooks.

Usage::

    from cua_sandbox.sync import sandbox, localhost, Image

    # Blocking sandbox
    with sandbox(local=True) as sb:
        sb.mouse.click(100, 200)
        img = sb.screenshot()

    # Blocking localhost
    with localhost() as host:
        host.mouse.click(100, 200)
"""

from __future__ import annotations

import asyncio
from contextlib import contextmanager
from typing import Any, Iterator, Optional

from cua_sandbox.image import Image
from cua_sandbox.localhost import Localhost as _AsyncLocalhost
from cua_sandbox.sandbox import Sandbox as _AsyncSandbox


def _get_or_create_loop() -> asyncio.AbstractEventLoop:
    """Get the running event loop, or create a new one."""
    try:
        loop = asyncio.get_running_loop()
        return loop
    except RuntimeError:
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        return loop


def _run(coro: Any) -> Any:
    """Run a coroutine synchronously."""
    try:
        asyncio.get_running_loop()
        # We're inside an existing event loop (e.g. Jupyter) — use nest_asyncio pattern
        import nest_asyncio

        nest_asyncio.apply()
        return asyncio.get_event_loop().run_until_complete(coro)
    except RuntimeError:
        return asyncio.run(coro)


class _SyncProxy:
    """Wraps an async object and makes attribute access synchronous."""

    def __init__(self, async_obj: Any):
        self._async_obj = async_obj

    def __getattr__(self, name: str) -> Any:
        attr = getattr(self._async_obj, name)
        if asyncio.iscoroutinefunction(attr):

            def sync_wrapper(*args: Any, **kwargs: Any) -> Any:
                return _run(attr(*args, **kwargs))

            return sync_wrapper
        # If the attribute is an interface object, wrap it too
        if hasattr(attr, "_t"):  # Interface objects have _t (transport)
            return _SyncProxy(attr)
        return attr

    def __repr__(self) -> str:
        return f"Sync({self._async_obj!r})"


@contextmanager
def sandbox(
    *,
    local: bool = False,
    ws_url: Optional[str] = None,
    api_key: Optional[str] = None,
    image: Optional[Image] = None,
    name: Optional[str] = None,
) -> Iterator[_SyncProxy]:
    """Synchronous context manager yielding a sync-wrapped Sandbox."""
    sb = _run(_AsyncSandbox._create(local=local, ws_url=ws_url, api_key=api_key, name=name))
    proxy = _SyncProxy(sb)
    try:
        yield proxy
    finally:
        _run(sb.disconnect())


@contextmanager
def localhost() -> Iterator[_SyncProxy]:
    """Synchronous context manager yielding a sync-wrapped Localhost."""
    host = _AsyncLocalhost()
    _run(host._connect())
    proxy = _SyncProxy(host)
    try:
        yield proxy
    finally:
        _run(host.disconnect())
