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
from cua_sandbox.pool import Pool as _AsyncPool
from cua_sandbox.sandbox import Sandbox as _AsyncSandbox
from fleet_sdk import CreatePoolRequest


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


class Pool:
    """Blocking facade for :class:`cua_sandbox.Pool`.

    ``Pool.reconcile`` and ``pool.claim`` use the same Fleet lifecycle as the
    async API, but yield synchronous sandbox interface methods for scripts and
    notebooks.
    """

    def __init__(self, async_pool: _AsyncPool) -> None:
        self._async_pool = async_pool

    @property
    def name(self) -> str:
        return self._async_pool.name

    @classmethod
    def reconcile(cls, request: CreatePoolRequest) -> "Pool":
        """Synchronously create or update a Fleet pool."""
        return cls(_run(_AsyncPool.reconcile(request)))

    @contextmanager
    def claim(self) -> Iterator[_SyncProxy]:
        """Synchronously lease a sandbox and release the claim on exit."""
        context = self._async_pool.claim()
        sandbox = _run(context.__aenter__())
        try:
            yield _SyncProxy(sandbox)
        except BaseException as error:
            _run(context.__aexit__(type(error), error, error.__traceback__))
            raise
        else:
            _run(context.__aexit__(None, None, None))


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
