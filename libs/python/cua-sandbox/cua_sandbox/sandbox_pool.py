"""Pools: sandboxes of one shape kept ready on this machine.

A create from a pool takes one of its ready sandboxes in about 0.1 s
instead of booting one. ``cua daemon`` keeps the pools (start it with
``cua daemon start``); they survive its restarts.

    from cua_sandbox import Image, Sandbox, create_pool

    pool = await create_pool(Image.linux(), name="agents", min=2, max=8)
    sb = await pool.create()                      # or Sandbox.create(pool=pool)
    sb2 = await Sandbox.create(pool="agents")     # by name works too

* ``min``: sandboxes kept ready at all times; one starts again as soon as
  one is created from the pool.
* ``max``: the most kept ready (default ``min``), not counting those
  created from the pool. Above ``min``, more are kept ready only while
  creates come faster than sandboxes start.
* ``idle``: after this long without a create, the pool keeps none ready
  until its next create (which starts on the default path and refills it).

Without a pool, the daemon keeps sandboxes ready only where repeated creates
of a shape pay for it (``cua daemon status`` says why); a pool asks for them
explicitly and bypasses that policy. Pools run on this machine only today
(``on="local"``); another location raises :class:`~cua_sandbox.Unsupported`.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import timedelta
from typing import TYPE_CHECKING, Any, AsyncIterator, Optional, Union

if TYPE_CHECKING:
    from cua_sandbox.image import Image
    from cua_sandbox.sandbox import Sandbox

Seconds = Union[float, timedelta, None]


def _translate(error: BaseException) -> BaseException:
    """The cua-sandbox exception for a native error from the pool API."""
    from cua_sandbox._placement import translate
    from cua_sandbox._sdk import Unsupported, native

    try:
        cua_error = native().CuaError
    except Exception:  # noqa: BLE001 - no SDK: nothing to translate
        return error
    if isinstance(error, (cua_error.Unsupported, cua_error.DaemonNotRunning)):
        return Unsupported(str(error))
    return translate(error)


async def _call(coro: Any) -> Any:
    try:
        return await coro
    except Exception as error:  # noqa: BLE001 - re-raised, typed when known
        typed = _translate(error)
        if typed is error:
            raise
        raise typed from error


def _sandboxes() -> Any:
    from cua_sandbox._sdk import daemon_runtime

    return daemon_runtime().sandboxes()


@dataclass(frozen=True)
class SandboxPool:
    """A pool (see :mod:`cua_sandbox.sandbox_pool`), with what it held when
    read. Make one with :func:`create_pool`."""

    name: str
    image: str
    #: Where its sandboxes run (``"local"``).
    location: str
    #: Kind and runtime as given (``""``: auto).
    kind: str
    runtime: str
    cpus: int
    memory_mb: int
    min: int
    max: int
    #: Idle time after which it keeps none ready (``None``: never).
    idle: Optional[timedelta]
    #: Sandboxes ready and starting when it was read.
    ready: int
    starting: int
    #: Why it holds what it holds, or why it cannot.
    status: str

    @classmethod
    def _of(cls, p: Any) -> "SandboxPool":
        idle_ms = getattr(p, "idle_ms", None)
        return cls(
            name=p.name,
            image=p.image,
            location=p.location,
            kind=p.kind,
            runtime=p.runtime,
            cpus=int(p.cpus),
            memory_mb=int(p.memory_mb),
            min=int(p.min),
            max=int(p.max),
            idle=timedelta(milliseconds=int(idle_ms)) if idle_ms is not None else None,
            ready=int(p.ready),
            starting=int(p.starting),
            status=p.status,
        )

    async def create(self, name: Optional[str] = None, **kwargs: Any) -> "Sandbox":
        """A sandbox from this pool (``Sandbox.create(pool=self, ...)``)."""
        from cua_sandbox.sandbox import Sandbox

        return await Sandbox.create(pool=self, name=name, **kwargs)

    def ephemeral(self, **kwargs: Any) -> "AsyncIterator[Sandbox]":
        """``async with pool.ephemeral() as sb``: a sandbox from this pool,
        deleted on exit."""
        from cua_sandbox.sandbox import Sandbox

        return Sandbox.ephemeral(pool=self, **kwargs)

    async def refresh(self) -> "SandboxPool":
        """This pool read again (what it holds now)."""
        p = await get_pool(self.name)
        if p is None:
            raise LookupError(f"pool {self.name!r} no longer exists")
        return p

    async def delete(self) -> bool:
        """Deletes this pool and its ready sandboxes (sandboxes created from
        it stay). Whether it existed."""
        return await delete_pool(self.name)


def _seconds_ms(idle: Seconds) -> Optional[int]:
    if idle is None:
        return None
    seconds = idle.total_seconds() if isinstance(idle, timedelta) else idle
    if isinstance(seconds, bool) or not isinstance(seconds, (int, float)) or seconds <= 0:
        raise ValueError("idle must be a positive number of seconds or a timedelta")
    return int(seconds * 1000)


async def create_pool(
    image: "Image | str",
    *,
    name: Optional[str] = None,
    min: int = 1,
    max: Optional[int] = None,
    idle: Seconds = None,
    on: str = "local",
    kind: Optional[str] = None,
    runtime: Optional[str] = None,
    cpu: Optional[int] = None,
    memory: "str | int | None" = None,
    memory_mb: Optional[int] = None,
    env: Optional[dict[str, str]] = None,
    services: Optional[dict[str, int]] = None,
) -> SandboxPool:
    """Creates pool ``name`` of ``image`` sandboxes (``None``: a generated
    name), or updates the pool of that name. ``kind``, ``runtime``, ``cpu``,
    ``memory``, ``env`` and ``services`` mean what they mean in
    :meth:`Sandbox.create`. Returns at once; its sandboxes start in the
    background (see the module docs for ``min``, ``max`` and ``idle``)."""
    from cua_sandbox._sdk import native
    from cua_sandbox.image import Image, registry_image
    from cua_sandbox.options import parse_memory

    if isinstance(min, bool) or not isinstance(min, int) or min < 0:
        raise ValueError("min must be a non-negative integer")
    if max is not None and (isinstance(max, bool) or not isinstance(max, int) or max < min):
        raise ValueError("max must be an integer of at least min")
    if memory is not None:
        if memory_mb is not None:
            raise ValueError("pass memory= or memory_mb=, not both")
        memory_mb = parse_memory(memory)
    os_type = None
    if isinstance(image, Image):
        ref = registry_image(image)
        if not ref:
            raise ValueError(
                "a pool needs a registry image (Image.from_registry(...), Image.linux())"
            )
        kind = kind or image.kind
        os_type = image.os_type
    else:
        ref = str(image)
    options = native().PoolCreateOptions(
        name=name,
        on=on,
        kind=kind,
        runtime=runtime,
        os=os_type,
        cpus=cpu,
        memory_mb=memory_mb,
        env=dict(env or {}),
        services={k: int(v) for k, v in (services or {}).items()},
        min=min,
        max=max,
        idle_ms=_seconds_ms(idle),
    )
    return SandboxPool._of(await _call(_sandboxes().create_pool(ref, options)))


async def get_pool(name: str) -> Optional[SandboxPool]:
    """Pool ``name`` with what it holds now, or ``None`` when there is none."""
    from cua_sandbox._sdk import is_not_found

    try:
        return SandboxPool._of(await _call(_sandboxes().get_pool(name)))
    except Exception as error:  # noqa: BLE001 - only "no such pool" is None
        if is_not_found(error) or is_not_found(error.__cause__ or error):
            return None
        raise


async def list_pools() -> list[SandboxPool]:
    """Every pool, with what each holds now."""
    return [SandboxPool._of(p) for p in await _call(_sandboxes().list_pools())]


async def delete_pool(name: str) -> bool:
    """Deletes pool ``name`` and its ready sandboxes (sandboxes created from
    it stay). Whether it existed."""
    return bool(await _call(_sandboxes().delete_pool(name)))
