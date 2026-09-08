"""Optional canonical Cua Driver connections over Fleet named services."""

from __future__ import annotations

import asyncio
import importlib
import json
import logging
import re
import uuid
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Any, AsyncIterator

from cua_sandbox.transport.fleet import FleetTransport

if TYPE_CHECKING:
    from cua_driver import CuaDriver

_REQUEST_LIMIT = 1024 * 1024
_RESPONSE_LIMIT = 16 * 1024 * 1024
_TOKEN = re.compile(r"[A-Za-z0-9_-]{1,256}\Z")
_CLEANUP_TIMEOUT = 2.0
logger = logging.getLogger(__name__)


async def _bounded_cleanup(awaitable: Any) -> bool:
    """Bound best-effort cleanup, including callbacks that ignore cancellation."""
    task = asyncio.ensure_future(awaitable)

    def consume_result(done: asyncio.Future) -> None:
        if not done.cancelled():
            done.exception()

    task.add_done_callback(consume_result)
    done, _ = await asyncio.wait({task}, timeout=_CLEANUP_TIMEOUT)
    if task not in done:
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

    Each connection is a host-bound Standard session. Use typed inputs with
    ``session=None`` where optional. For required session fields, obtain the
    bound name with ``sandbox.driver.session_name(driver)``. Creating or
    rebinding trusted sessions is unsupported. Remote cleanup is best effort
    with a bounded timeout; failures warn and do not block claim release.
    """

    def __init__(self, transport: Any):
        self._transport = transport
        # Identity belongs to this bound Fleet client lifecycle, never guest input.
        self._principal = uuid.uuid4().hex
        self._lock = asyncio.Lock()
        self._connections: dict[Any, Any] = {}
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
    async def connect(self, *, service: str = "driver") -> AsyncIterator[CuaDriver]:
        """Yield the canonical ``cua_driver.CuaDriver`` and close its session on exit."""
        self._check_loop()
        self._loop = asyncio.get_running_loop()
        async with self._lock:
            if self._closed:
                raise DriverConnectionError("Sandbox Driver accessor is disconnected")
            if not isinstance(self._transport, FleetTransport):
                raise DriverConnectionError("Typed Driver requires a Fleet transport")
            if not self._transport._connected:
                raise DriverConnectionError("Fleet transport is disconnected")
            if service not in self._transport._bound.services:
                raise DriverConnectionError(
                    "Fleet sandbox does not expose the requested Driver service"
                )
            sdk = _sdk()
            channel = _channel(sdk, self._transport, service, self._principal)
            try:
                await channel.open()
                driver = sdk.connect_remote_channel(channel)
            except BaseException:
                await channel.close()
                raise
            self._connections[channel] = driver
        try:
            yield driver
        finally:
            await self._close_connection(channel)

    async def _close_connection(self, channel: Any) -> None:
        self._check_loop()
        async with self._lock:
            driver = self._connections.pop(channel, None)
            if driver is not None:
                channel.closed = True
                await _bounded_cleanup(channel.close())
                await _bounded_cleanup(driver.shutdown())

    async def close(self) -> None:
        """Invalidate all connections before the owning transport is closed."""
        self._check_loop()
        async with self._lock:
            self._closed = True
            for channel in self._connections:
                channel.closed = True
        for channel in list(self._connections):
            try:
                await self._close_connection(channel)
            except Exception:
                logger.warning("Driver cleanup failed; remote session cleanup is unconfirmed")


def _channel(sdk: Any, transport: FleetTransport, service: str, principal: str) -> Any:
    class Channel(sdk.ForeignDriverEnvelopeChannel):
        def __init__(self):
            self.connection_id = None
            self.generation = None
            self.public_session = None
            self.capabilities = None
            self.closed = False
            self.cleanup_confirmed = False
            self._close_task = None
            self.cancelled: set[str] = set()

        def fail(self, reason: str):
            return sdk.ForeignDriverChannelError.Failed(reason)

        async def request(self, method: str, suffix: str = "", body: Any = None):
            path = "/v1/connections"
            headers = None
            if self.connection_id is not None:
                path += "/" + self.connection_id + suffix
                headers = {"X-Cua-Driver-Generation": self.generation}
            try:
                response = await transport.request_service(
                    service, method=method, path=path, json_body=body, headers=headers
                )
            except Exception as error:
                if getattr(error, "status", None) in (401, 403):
                    raise self.fail("Driver service authorization denied") from None
                raise self.fail("Driver service transport failed; completion is unknown") from None
            status = response.status_code
            if status in (401, 403):
                raise self.fail("Driver service authorization denied")
            if status in (404, 409):
                reason = (
                    "Driver connection is stale"
                    if self.connection_id
                    else "Driver service unavailable"
                )
                if self.connection_id:
                    self.closed = True
                raise self.fail(reason)
            if status in (400, 405, 415, 422, 426):
                raise self.fail("Driver service protocol is incompatible")
            if not 200 <= status < 300:
                raise self.fail("Driver service request failed; completion is unknown")
            if len(response.content) > _RESPONSE_LIMIT:
                raise self.fail("Driver service response exceeds the size limit")
            return response

        async def open(self):
            response = await self.request("POST", body={})
            try:
                data = response.json()
                connection_id, generation = data["connection_id"], data["generation"]
                if not all(
                    isinstance(v, str) and _TOKEN.fullmatch(v) for v in (connection_id, generation)
                ):
                    raise ValueError
                self.connection_id, self.generation = connection_id, generation
                caps = data["capabilities"]
                minimum, maximum = (
                    caps["minimum_envelope_version"],
                    caps["maximum_envelope_version"],
                )
                if (
                    type(minimum) is not int
                    or type(maximum) is not int
                    or not 0 <= minimum <= 1 <= maximum <= 4294967295
                    or caps["supports_cancellation"] is not True
                ):
                    raise ValueError
                if not isinstance(data["public_session"], str) or not data["public_session"]:
                    raise ValueError
                self.public_session = data["public_session"]
                self.capabilities = sdk.ForeignDriverChannelCapabilities(
                    minimum_envelope_version=minimum,
                    maximum_envelope_version=maximum,
                    supports_cancellation=True,
                )
            except (ValueError, TypeError, KeyError):
                raise self.fail("Driver service negotiation is malformed or incompatible") from None

        def identity(self):
            return sdk.ForeignDriverChannelIdentity(
                authenticated_principal=principal, connection_generation=self.generation
            )

        async def negotiate(self):
            if self.closed:
                raise self.fail("Driver connection is closed")
            return self.capabilities

        async def bind_session(self, options):
            raise self.fail(
                "Fleet Driver connections use a host-bound Standard session; rebinding is unsupported"
            )

        async def exchange(self, request):
            if self.closed or request.request_id in self.cancelled:
                raise self.fail("Driver connection is closed or request was cancelled")
            try:
                body = {
                    "envelope_version": request.envelope_version,
                    "request_id": request.request_id,
                    "operation": request.operation,
                    "name": request.name,
                    "arguments": (
                        json.loads(request.arguments_json)
                        if request.arguments_json is not None
                        else None
                    ),
                    "deadline_unix_ms": request.deadline_unix_ms,
                }
                if len(json.dumps(body, allow_nan=False).encode()) > _REQUEST_LIMIT:
                    raise ValueError
            except (ValueError, TypeError):
                raise self.fail("Driver request is malformed or exceeds the size limit") from None
            try:
                response = await self.request("POST", "/exchange", body)
                return await self.decode_response(request, response)
            except BaseException:
                self.closed = True
                await self.close()
                raise

        async def decode_response(self, request, response):
            if self.closed or request.request_id in self.cancelled:
                raise self.fail(
                    "Driver response arrived after close or cancellation; completion is unknown"
                )
            try:
                data = response.json()
                if (
                    type(data["envelope_version"]) is not int
                    or data["envelope_version"] != request.envelope_version
                    or not isinstance(data["request_id"], str)
                    or data["request_id"] != request.request_id
                    or type(data["ok"]) is not bool
                    or type(data["completion_known"]) is not bool
                    or any(
                        data.get(key) is not None and not isinstance(data[key], str)
                        for key in ("error", "error_code")
                    )
                    or (
                        data.get("error_code") is not None
                        and not _TOKEN.fullmatch(data["error_code"])
                    )
                ):
                    raise ValueError
                result = sdk.ForeignDriverResponseEnvelope(
                    envelope_version=data["envelope_version"],
                    request_id=data["request_id"],
                    ok=data["ok"],
                    result_json=(
                        json.dumps(data["result"], allow_nan=False) if "result" in data else None
                    ),
                    error="Driver operation failed" if data.get("error") is not None else None,
                    error_code=data.get("error_code"),
                    completion_known=data["completion_known"],
                )
                if not data["completion_known"]:
                    self.closed = True
                    await self.close()
                return result
            except (ValueError, TypeError, KeyError):
                raise self.fail("Driver response is malformed; completion is unknown") from None

        async def cancel(self, request_id):
            self.cancelled.add(request_id)
            if not self.closed:
                self.closed = True
                try:
                    await _bounded_cleanup(
                        self.request("POST", "/cancel", {"request_id": request_id})
                    )
                finally:
                    await self.close()

        async def close(self):
            self.closed = True
            if self._close_task is None:

                async def cleanup():
                    if self.connection_id is not None:
                        self.cleanup_confirmed = await _bounded_cleanup(self.request("DELETE"))

                self._close_task = asyncio.create_task(cleanup())
            await asyncio.shield(self._close_task)

    return Channel()
