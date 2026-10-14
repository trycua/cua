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
from typing import Any, Callable, Iterator, Optional

from cua_sandbox._sdk import ENV_SERVICE
from cua_sandbox.image import Image
from cua_sandbox.pool import Pool as _AsyncPool
from cua_sandbox.pool import Template as _AsyncTemplate
from cua_sandbox.sandbox import Sandbox as _AsyncSandbox
from fleet_sdk import (
    ClaimSpec,
    CreatePoolRequest,
    CreateTemplateRequest,
)


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


class Template:
    """Blocking facade for :class:`cua_sandbox.Template`."""

    def __init__(self, async_template: _AsyncTemplate) -> None:
        self._async_template = async_template

    @property
    def name(self) -> str:
        return self._async_template.name

    @classmethod
    def reconcile(cls, request: CreateTemplateRequest) -> "Template":
        """Synchronously create or update a Fleet sandbox template."""
        return cls(_run(_AsyncTemplate.reconcile(request)))


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

    @classmethod
    def get(cls, name: str) -> "Pool":
        """Synchronously fetch an existing Fleet pool without changing it."""
        return cls(_run(_AsyncPool.get(name)))

    @classmethod
    def apply(cls, *args: Any, **kwargs: Any) -> "Pool":
        """Synchronously apply a Fleet pool: ``Pool.apply(name, spec, options)``
        (the deprecated ``Pool.apply(image, name=..., ...)`` form works too;
        see ``cua_sandbox.Pool.apply``)."""
        return cls(_run(_AsyncPool.apply(*args, **kwargs)))

    @staticmethod
    def export(name: str, *, terraform: bool = False) -> Any:
        """Synchronously read a pool back (see ``cua_sandbox.Pool.export``)."""
        return _run(_AsyncPool.export(name, terraform=terraform))

    def delete(self) -> None:
        """Synchronously delete this Fleet pool."""
        _run(self._async_pool.delete())

    @contextmanager
    def claim(
        self,
        *,
        spec: ClaimSpec | None = None,
        name: str | None = None,
        service: str = ENV_SERVICE,
        time_to_start: float | None = None,
        ttl_seconds_after_created: int | None = None,
        claim_token: str | None = None,
    ) -> Iterator[_SyncProxy]:
        """Synchronously claim a sandbox and release it on exit."""
        context = self._async_pool.claim(
            spec=spec,
            name=name,
            service=service,
            time_to_start=time_to_start,
            ttl_seconds_after_created=ttl_seconds_after_created,
            claim_token=claim_token,
        )
        sandbox = _run(context.__aenter__())
        try:
            yield _SyncProxy(sandbox)
        except BaseException as error:
            _run(context.__aexit__(type(error), error, error.__traceback__))
            raise
        else:
            _run(context.__aexit__(None, None, None))


class Sandbox:
    """Blocking facade for :class:`cua_sandbox.Sandbox` factories.

    Each method takes the same arguments as its async counterpart and returns
    sync-wrapped sandboxes. Managed Fleet claims keep renewing in the
    background between calls, exactly as with the async API.
    """

    @staticmethod
    def create(image: Optional[Image] = None, **kwargs: Any) -> _SyncProxy:
        return _SyncProxy(_run(_AsyncSandbox.create(image, **kwargs)))

    @staticmethod
    def connect(name: Optional[str] = None, **kwargs: Any) -> _SyncProxy:
        async def connect() -> Any:
            return await _AsyncSandbox.connect(name, **kwargs)

        return _SyncProxy(_run(connect()))

    @staticmethod
    @contextmanager
    def ephemeral(image: Optional[Image] = None, **kwargs: Any) -> Iterator[_SyncProxy]:
        context = _AsyncSandbox.ephemeral(image, **kwargs)
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
    warm: Optional[bool] = None,
    max_pool_size: Optional[int] = None,
    claim_ttl: Any = None,
    progress: Optional[Callable[[Any], Any]] = None,
) -> Iterator[_SyncProxy]:
    """Synchronous context manager yielding a sync-wrapped Sandbox.

    Mirrors :func:`cua_sandbox.sandbox`: an ephemeral sandbox (the default when
    ``image`` is given) is destroyed on exit, anything else is disconnected.
    Fleet registry images come from the account's managed pool (see
    :meth:`cua_sandbox.Sandbox.create` for ``on``/``local``, ``kind``,
    ``runtime``, ``warm``, ``max_pool_size``, ``claim_ttl`` and ``progress``).
    """
    from cua_sandbox import _placement
    from cua_sandbox.sandbox import _cloud_only_args, _place_new

    engine, hint = None, None
    if image is not None and not (url or http_url or ws_url):
        place, image = _place_new(
            image,
            on=on,
            local=local,
            kind=kind,
            runtime=runtime,
            cloud=None,
            cloud_only=_cloud_only_args(
                warm=warm,
                max_pool_size=max_pool_size,
                claim_ttl=claim_ttl,
                api_key=api_key,
                progress=progress,
            ),
        )
        local, engine, runtime = place.local, place.runtime, place.legacy_runtime
        hint = place.cloud_default_hint()
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
            warm=warm,
            max_pool_size=max_pool_size,
            claim_ttl=claim_ttl,
            progress=progress,
            engine=engine,
            hint=hint,
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
