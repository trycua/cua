"""Optional canonical Cua Driver connections for a sandbox.

``sb.driver.connect()`` yields the generated, typed ``cua_driver.CuaDriver``
for the guest's desktop. It rides cua-spacesd, the default for every sandbox
the cua SDK drives: the typed-envelope MCP extension on the spacesd's ``/mcp``
route, reached through the cua SDK's env client (``SpacesdClient.http``) with
the sandbox's own endpoint and credentials, so direct and local sandboxes both
work.
"""

from __future__ import annotations

import asyncio
import importlib
import logging
import uuid
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Any, AsyncIterator, Literal

from cua_sandbox._sdk import ENV_SERVICE
from cua_sandbox.transport.env import EnvTransport

if TYPE_CHECKING:
    from cua_driver import CuaDriver

_CLEANUP_TIMEOUT = 2.0
logger = logging.getLogger(__name__)


def _consume_result(done: asyncio.Future) -> None:
    if not done.cancelled():
        done.exception()


async def _bounded_cleanup(awaitable: Any, *, cancel_on_timeout: bool = True) -> bool:
    """Bound best-effort cleanup, including callbacks that ignore cancellation."""
    task = asyncio.ensure_future(awaitable)

    task.add_done_callback(_consume_result)
    done, _ = await asyncio.wait({task}, timeout=_CLEANUP_TIMEOUT)
    if task not in done:
        if cancel_on_timeout:
            task.cancel()
        logger.warning("Driver cleanup timed out; remote session cleanup is unconfirmed")
        return False
    if task.cancelled() or task.exception() is not None:
        logger.warning("Driver cleanup failed; remote session cleanup is unconfirmed")
        return False
    return True


class DriverConnectionError(RuntimeError):
    """Sanitized failure to establish or use a sandbox Driver service."""


def _sdk() -> Any:
    try:
        sdk = importlib.import_module("cua_driver")
        if not hasattr(sdk, "connect_remote_channel"):
            raise ImportError
        return sdk
    except ImportError:
        raise DriverConnectionError(
            "Install a cua-driver version providing connect_remote_channel to use sandbox.driver"
        ) from None


class Driver:
    """Accessor for optional typed Driver sessions; does not replace Sandbox transport.

    Each connection is a host-bound session with a launcher-selected permission
    mode (Standard by default). Remote clients cannot select its authority.
    Use typed inputs with ``session=None`` where optional. For required session
    fields, obtain the bound name with ``sandbox.driver.session_name(driver)``. Creating or
    rebinding trusted sessions is unsupported. Remote cleanup is best effort
    with a bounded timeout; failures warn and do not block the sandbox's release.
    """

    def __init__(self, transport: Any):
        self._transport = transport
        # Identity belongs to this sandbox connection, never guest input.
        self._principal = uuid.uuid4().hex
        self._lock = asyncio.Lock()
        self._connections: dict[Any, Any] = {}
        self._opening: dict[Any, asyncio.Task] = {}
        self._closing: dict[Any, asyncio.Task] = {}
        self._closed = False
        self._loop: asyncio.AbstractEventLoop | None = None

    def _check_loop(self) -> None:
        if self._loop is not None and self._loop is not asyncio.get_running_loop():
            raise DriverConnectionError(
                "Sandbox Driver connections belong to a different event loop"
            )

    def session_name(self, driver: CuaDriver) -> str:
        """Return this active connection's host-bound public session label.

        This label is not a credential or permission grant. It can populate
        canonical typed inputs whose ``session`` field is required.
        """
        self._check_loop()
        for channel, connection in self._connections.items():
            if connection is driver and not channel.closed:
                return channel.public_session
        raise DriverConnectionError("Driver connection is inactive or belongs to another sandbox")

    @asynccontextmanager
    async def connect(
        self,
        *,
        service: str | None = None,
        transport: Literal["envelope", "mcp"] | None = None,
    ) -> AsyncIterator[CuaDriver]:
        """Yield the canonical Driver; the receiver must support typed envelopes.

        This uses cua-spacesd's ``/mcp`` (``service="env"``,
        ``transport="mcp"``) through the cua SDK, and verifies the
        ``ai.cua.driver.envelopes`` extension before opening a receiver.
        Named Driver services were a Cua Cloud feature: any other ``service``
        or ``transport="envelope"`` raises :class:`DriverConnectionError`.
        """
        self._check_loop()
        if transport not in (None, "envelope", "mcp"):
            raise DriverConnectionError("Driver transport must be 'envelope' or 'mcp'")
        service, transport = self._resolve(service, transport)
        self._loop = asyncio.get_running_loop()
        async with self._lock:
            if self._closed:
                raise DriverConnectionError("Sandbox Driver accessor is disconnected")
            if not self._transport._connected:
                raise DriverConnectionError("Sandbox transport is disconnected")
            carrier = _EnvMcpCarrier(self._transport)
            sdk = _sdk()
            from cua_sandbox.interfaces._driver_mcp import shared_channel

            channel = shared_channel(sdk, carrier, service, self._principal)

            async def open_channel():
                try:
                    await channel.open()
                finally:
                    # A cancelled carrier may still return an allocated session.
                    if channel.closed:
                        await channel.close()

            opening = asyncio.create_task(open_channel())
            opening.add_done_callback(_consume_result)
            self._opening[channel] = opening
            self._connections[channel] = None
        try:
            await asyncio.shield(opening)
            async with self._lock:
                if self._closed or channel.closed:
                    raise DriverConnectionError("Sandbox Driver accessor is disconnected")
                driver = channel.driver()
                self._connections[channel] = driver
            yield driver
        finally:
            await self._close_connection(channel)

    def _resolve(
        self, service: str | None, transport: str | None
    ) -> tuple[str, Literal["envelope", "mcp"]]:
        """Pick the carrier. Only cua-spacesd sandboxes have one."""
        if not isinstance(self._transport, EnvTransport):
            raise DriverConnectionError("Typed Driver requires a cua-spacesd sandbox transport")
        if transport == "envelope":
            raise DriverConnectionError("cua-spacesd carries the typed Driver over MCP only")
        if service not in (None, ENV_SERVICE):
            raise DriverConnectionError(
                "Named Driver services are not available; cua-spacesd carries the typed Driver"
            )
        return ENV_SERVICE, "mcp"

    async def _close_connection(self, channel: Any) -> None:
        self._check_loop()
        async with self._lock:
            task = self._closing.get(channel)
            if task is None and channel in self._connections:
                driver = self._connections.pop(channel)
                opening = self._opening.pop(channel)
                channel.closed = True

                async def cleanup():
                    if getattr(channel, "shared_mcp", False):
                        await channel.close()
                        return
                    if not opening.done():
                        await _bounded_cleanup(opening)
                    await channel.close()
                    if driver is not None:
                        await _bounded_cleanup(driver.shutdown())

                task = asyncio.create_task(cleanup())
                self._closing[channel] = task
                task.add_done_callback(_consume_result)
                task.add_done_callback(lambda done: self._closing.pop(channel, None))
        if task is not None:
            await asyncio.shield(task)

    async def close(self) -> None:
        """Invalidate all connections before the owning transport is closed."""
        self._check_loop()
        async with self._lock:
            self._closed = True
            for channel in self._connections:
                channel.closed = True

        async def close_channel(channel):
            try:
                await self._close_connection(channel)
            except Exception:
                logger.warning("Driver cleanup failed; remote session cleanup is unconfirmed")

        await asyncio.gather(
            *(close_channel(channel) for channel in set(self._connections) | set(self._closing))
        )


class _EnvMcpCarrier:
    """``request_service`` over the cua SDK's env client (``SpacesdClient.http``).

    Gives the shared MCP channel its byte transport, addressed to
    cua-spacesd's own ``/mcp`` with the sandbox's env credentials. The service
    name is ignored: there is one spacesd per sandbox.
    """

    def __init__(self, transport: EnvTransport):
        self._transport = transport

    @property
    def _connected(self) -> bool:
        return bool(self._transport._connected)

    async def request_service(
        self,
        name: str,
        *,
        method: str,
        path: str,
        body: bytes | None = None,
        headers: Any = None,
        timeout: float | None = None,
        max_response_bytes: int | None = None,
        json_body: Any = None,
    ) -> Any:
        import httpx
        from cua_sandbox._sdk import native

        if json_body is not None:
            raise DriverConnectionError("cua-spacesd MCP requests carry raw bytes")
        header_items = headers.items() if isinstance(headers, dict) else (headers or [])
        env = await self._transport.spacesd()
        types = native()
        reply = await env.http(
            types.SpacesdHttpRequest(
                method=method,
                path=path,
                headers=[types.SpacesdHttpHeader(name=k, value=v) for k, v in header_items],
                body=body or b"",
                timeout_ms=None if timeout is None else max(1, int(timeout * 1000)),
                max_response_bytes=max_response_bytes,
            )
        )
        return httpx.Response(
            reply.status,
            headers=[(h.name, h.value) for h in reply.headers],
            content=bytes(reply.body),
            request=httpx.Request(method, f"https://env.invalid{path}"),
        )
