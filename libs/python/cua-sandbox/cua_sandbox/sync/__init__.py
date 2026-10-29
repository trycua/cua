"""Synchronous API wrappers for scripts and notebooks.

Usage::

    from cua_sandbox.sync import sandbox, Image

    # Blocking sandbox
    with sandbox(local=True) as sb:
        sb.mouse.click(100, 200)
        img = sb.screenshot()
"""

from __future__ import annotations

import asyncio
from contextlib import contextmanager
from typing import Any, Iterator, Optional

from cua_sandbox.image import Image
from cua_sandbox.sandbox import Sandbox as _AsyncSandbox
from cua_sandbox.sandbox_pool import SandboxPool as _AsyncSandboxPool


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
        if name == "service" and callable(attr):
            # sb.service("mcp").request(...) / .url() / .public_url()
            return lambda *args, **kwargs: _SyncProxy(attr(*args, **kwargs))
        return attr

    def __repr__(self) -> str:
        return f"Sync({self._async_obj!r})"


def _unwrap_pool(kwargs: dict[str, Any]) -> dict[str, Any]:
    """``pool=`` given a blocking :class:`SandboxPool`: its async pool."""
    pool = kwargs.get("pool")
    if isinstance(pool, SandboxPool):
        kwargs = {**kwargs, "pool": pool._pool}
    return kwargs


class Sandbox:
    """Blocking facade for :class:`cua_sandbox.Sandbox` factories.

    Each method takes the same arguments as its async counterpart and returns
    sync-wrapped sandboxes.
    """

    @staticmethod
    def create(image: Optional[Image] = None, **kwargs: Any) -> _SyncProxy:
        return _SyncProxy(_run(_AsyncSandbox.create(image, **_unwrap_pool(kwargs))))

    @staticmethod
    def connect(name: Optional[str] = None, **kwargs: Any) -> _SyncProxy:
        async def connect() -> Any:
            return await _AsyncSandbox.connect(name, **kwargs)

        return _SyncProxy(_run(connect()))

    @staticmethod
    @contextmanager
    def ephemeral(image: Optional[Image] = None, **kwargs: Any) -> Iterator[_SyncProxy]:
        context = _AsyncSandbox.ephemeral(image, **_unwrap_pool(kwargs))
        sandbox = _run(context.__aenter__())
        try:
            yield _SyncProxy(sandbox)
        except BaseException as error:
            if not _run(context.__aexit__(type(error), error, error.__traceback__)):
                raise
        else:
            _run(context.__aexit__(None, None, None))

    @staticmethod
    def list(**kwargs: Any) -> Any:
        return _run(_AsyncSandbox.list(**kwargs))

    @staticmethod
    def get_info(name: str, **kwargs: Any) -> Any:
        return _run(_AsyncSandbox.get_info(name, **kwargs))

    @staticmethod
    def delete(name: str, **kwargs: Any) -> None:
        _run(_AsyncSandbox.delete(name, **kwargs))


class SandboxPool:
    """Blocking facade for :class:`cua_sandbox.SandboxPool`: the same
    fields, with blocking ``create``, ``ephemeral``, ``refresh`` and
    ``delete``."""

    def __init__(self, pool: _AsyncSandboxPool) -> None:
        self._pool = pool

    def __getattr__(self, name: str) -> Any:
        return getattr(self._pool, name)

    def __repr__(self) -> str:
        return repr(self._pool)

    def create(self, name: Optional[str] = None, **kwargs: Any) -> _SyncProxy:
        return Sandbox.create(pool=self._pool, name=name, **kwargs)

    @contextmanager
    def ephemeral(self, **kwargs: Any) -> Iterator[_SyncProxy]:
        with Sandbox.ephemeral(pool=self._pool, **kwargs) as sb:
            yield sb

    def refresh(self) -> "SandboxPool":
        return SandboxPool(_run(self._pool.refresh()))

    def delete(self) -> bool:
        return bool(_run(self._pool.delete()))


def create_pool(image: Any, **kwargs: Any) -> SandboxPool:
    """Blocking :func:`cua_sandbox.create_pool`."""
    from cua_sandbox import sandbox_pool

    return SandboxPool(_run(sandbox_pool.create_pool(image, **kwargs)))


def get_pool(name: str) -> Optional[SandboxPool]:
    """Blocking :func:`cua_sandbox.get_pool`."""
    from cua_sandbox import sandbox_pool

    p = _run(sandbox_pool.get_pool(name))
    return SandboxPool(p) if p is not None else None


def list_pools() -> list[SandboxPool]:
    """Blocking :func:`cua_sandbox.list_pools`."""
    from cua_sandbox import sandbox_pool

    return [SandboxPool(p) for p in _run(sandbox_pool.list_pools())]


def delete_pool(name: str) -> bool:
    """Blocking :func:`cua_sandbox.delete_pool`."""
    from cua_sandbox import sandbox_pool

    return bool(_run(sandbox_pool.delete_pool(name)))


@contextmanager
def sandbox(
    *,
    on: Optional[str] = None,
    local: Optional[bool] = None,
    kind: Optional[str] = None,
    ws_url: Optional[str] = None,
    http_url: Optional[str] = None,
    url: Optional[str] = None,
    token: Optional[str] = None,
    api_key: Optional[str] = None,
    image: Optional[Image] = None,
    runtime: Optional[Any] = None,
    name: Optional[str] = None,
    ephemeral: Optional[bool] = None,
) -> Iterator[_SyncProxy]:
    """Synchronous context manager yielding a sync-wrapped Sandbox.

    Mirrors :func:`cua_sandbox.sandbox`: an ephemeral sandbox (the default when
    ``image`` is given) is destroyed on exit, anything else is disconnected.
    See :meth:`cua_sandbox.Sandbox.create` for ``on``/``local``, ``kind`` and
    ``runtime``.
    """
    from cua_sandbox import _placement
    from cua_sandbox.sandbox import _place_new

    engine = None
    if image is not None and not (url or http_url or ws_url):
        place, image = _place_new(image, on=on, local=local, kind=kind, runtime=runtime)
        local, engine, runtime = place.local, place.runtime, place.legacy_runtime
    elif on is not None:
        local = _placement.resolve(on=on, local=local).local
    sb = _run(
        _AsyncSandbox._create(
            local=bool(local),
            ws_url=ws_url,
            http_url=http_url,
            url=url,
            token=token,
            api_key=api_key,
            image=image,
            runtime=runtime,
            name=name,
            ephemeral=ephemeral,
            engine=engine,
        )
    )
    proxy = _SyncProxy(sb)
    try:
        yield proxy
    finally:
        if sb._ephemeral:
            _run(sb.destroy())
        else:
            _run(sb.disconnect())
