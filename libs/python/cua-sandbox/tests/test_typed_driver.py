"""Fleet carrier contracts without requiring the optional native Driver library."""

import asyncio
import json
import sys
from types import SimpleNamespace

import httpx
import pytest
from cua_sandbox.interfaces.driver import DriverConnectionError
from cua_sandbox.sandbox import Sandbox
from cua_sandbox.transport.fleet import FleetTransport
from cua_sandbox.transport.fleet_cloud import FleetCloudTransport
from cua_sandbox.transport.local import LocalTransport


class ChannelError(Exception):
    pass


class CanonicalDriver:
    def __init__(self, channel):
        self.channel = channel
        self.shutdowns = 0

    async def shutdown(self):
        self.shutdowns += 1
        await self.channel.close()


@pytest.fixture
def native(monkeypatch):
    sdk = SimpleNamespace(
        CuaDriver=CanonicalDriver,
        ForeignDriverEnvelopeChannel=object,
        ForeignDriverChannelError=SimpleNamespace(Failed=ChannelError),
        ForeignDriverChannelCapabilities=SimpleNamespace,
        ForeignDriverChannelIdentity=SimpleNamespace,
        ForeignDriverResponseEnvelope=SimpleNamespace,
        connect_remote_channel=CanonicalDriver,
        _native=SimpleNamespace(uniffi_set_event_loop=lambda loop: None),
    )
    monkeypatch.setitem(sys.modules, "cua_driver", sdk)
    return sdk


def negotiation():
    return dict(
        connection_id="connection-1",
        generation="generation-1",
        public_session="session-1",
        capabilities=dict(
            minimum_envelope_version=1, maximum_envelope_version=1, supports_cancellation=True
        ),
    )


def envelope(**overrides):
    return SimpleNamespace(
        **dict(
            dict(
                envelope_version=1,
                request_id="request-1",
                operation="call",
                name="get_screen_size",
                arguments_json='{"session":null}',
                deadline_unix_ms=1900000000000,
            ),
            **overrides,
        )
    )


class Transport(FleetTransport):
    def __init__(self):
        super().__init__(
            sdk=None, bound=SimpleNamespace(services=["server", "driver"]), service_name="server"
        )
        self.events = []
        self.open_data = negotiation()
        self.response_data = dict(
            envelope_version=1,
            request_id="request-1",
            ok=True,
            result={"width": 1280},
            completion_known=True,
        )
        self.status = 200

    async def request_service(
        self, name, *, method, path, json_body=None, headers=None, timeout=None
    ):
        assert self._connected
        self.events.append((name, method, path, json_body, headers))
        data = self.open_data if path == "/v1/connections" else self.response_data
        return httpx.Response(self.status, json=data)

    async def disconnect(self):
        self.events.append("disconnect")
        await super().disconnect()


@pytest.fixture
async def sandbox(native):
    sb = Sandbox(Transport(), _telemetry_enabled=False)
    await sb._connect()
    yield sb
    await sb.disconnect()


async def test_canonical_driver_preserves_default_transport_and_pins_wire(sandbox):
    transport = sandbox._transport
    async with sandbox.driver.connect() as driver:
        assert isinstance(driver, CanonicalDriver)
        assert sandbox._transport is transport
        for interface in (sandbox.shell, sandbox.files, sandbox.mouse, sandbox.keyboard):
            assert interface._t is transport
        assert transport._service_name == "server"
        channel = driver.channel
        assert channel.public_session == "session-1"
        assert channel.identity().connection_generation == "generation-1"
        assert channel.identity().authenticated_principal != "session-1"
        assert (await channel.negotiate()).supports_cancellation
        result = await channel.exchange(envelope())
        assert json.loads(result.result_json) == {"width": 1280}
        _, method, path, body, headers = transport.events[-1]
        assert (method, path) == ("POST", "/v1/connections/connection-1/exchange")
        assert body["arguments"] == {"session": None}
        assert body["deadline_unix_ms"] == 1900000000000
        assert headers == {"X-Cua-Driver-Generation": "generation-1"}
    assert driver.shutdowns == 1
    assert transport.events[-1][1:3] == ("DELETE", "/v1/connections/connection-1")
    assert transport._connected


async def test_disconnect_closes_before_transport_and_context_exit_is_idempotent(sandbox):
    async with sandbox.driver.connect() as driver:
        await sandbox.disconnect()
        assert sandbox._transport.events[-2][1] == "DELETE"
        assert sandbox._transport.events[-1] == "disconnect"
        with pytest.raises(ChannelError, match="closed"):
            await driver.channel.exchange(envelope())
    assert driver.shutdowns == 1
    with pytest.raises(DriverConnectionError, match="disconnected"):
        async with sandbox.driver.connect():
            pass


async def test_claim_close_orders_driver_release_transport(sandbox):
    async def release():
        sandbox._transport.events.append("release")

    sandbox._claim_handle = SimpleNamespace(release=release, name=None)
    async with sandbox.driver.connect() as driver:
        await sandbox.close()
        assert sandbox._transport.events[-3][1] == "DELETE"
        assert sandbox._transport.events[-2:] == ["release", "disconnect"]
    assert driver.shutdowns == 1


async def test_pool_claim_context_closes_driver_before_claim_and_client(native, monkeypatch):
    from cua_sandbox import Pool

    from .test_pool import FakeFleetClient, fleet_pool

    events = []

    class Client(FakeFleetClient):
        async def wait_claim(self, claim):
            bound = await super().wait_claim(claim)
            bound.services.append("driver")
            return bound

        async def service_request(self, sandbox, service, path, request):
            events.append(request.method)
            return SimpleNamespace(status=200, headers=[], body=json.dumps(negotiation()).encode())

        async def delete_claim(self, claim):
            events.append("release")
            await super().delete_claim(claim)

        async def close(self):
            events.append("client-close")
            await super().close()

    client = Client()
    monkeypatch.setattr("cua_sandbox.pool._FleetClient", lambda: client)
    async with Pool(fleet_pool()).claim() as sb:
        context = sb.driver.connect()
        driver = await context.__aenter__()
    assert events == ["POST", "DELETE", "release", "client-close"]
    await context.__aexit__(None, None, None)
    assert driver.shutdowns == 1
    assert client.deleted_pools == []
    assert client.deleted_templates == []


async def test_all_connections_close_when_one_shutdown_fails(sandbox):
    first = sandbox.driver.connect()
    second = sandbox.driver.connect()
    driver = await first.__aenter__()
    other = await second.__aenter__()

    async def fail():
        raise RuntimeError("shutdown failed")

    driver.shutdown = fail
    await sandbox.disconnect()
    assert driver.channel.closed and other.channel.closed
    assert other.shutdowns == 1
    await first.__aexit__(None, None, None)
    await second.__aexit__(None, None, None)


async def test_oversized_request_is_not_dispatched(sandbox):
    async with sandbox.driver.connect() as driver:
        with pytest.raises(ChannelError, match="size limit"):
            await driver.channel.exchange(envelope(arguments_json=json.dumps("x" * 1024 * 1024)))
        assert len(sandbox._transport.events) == 1


@pytest.mark.parametrize(
    "status,reason",
    [
        (401, "authorization"),
        (403, "authorization"),
        (404, "stale"),
        (409, "stale"),
        (426, "incompatible"),
        (500, "unknown"),
    ],
)
async def test_exchange_errors_are_sanitized_and_never_replayed(sandbox, status, reason):
    async with sandbox.driver.connect() as driver:
        sandbox._transport.status = status
        sandbox._transport.response_data = {"error": "secret-desktop-content"}
        with pytest.raises(ChannelError, match=reason) as error:
            await driver.channel.exchange(envelope())
        assert "secret" not in str(error.value)
        assert len(sandbox._transport.events) == (3 if status in (404, 409) else 4)
        assert sandbox._transport.events[-1][1] == "DELETE"
        assert driver.channel.closed
        before = len(sandbox._transport.events)
        with pytest.raises(ChannelError, match="closed"):
            await driver.channel.exchange(envelope(request_id="next-request"))
        assert len(sandbox._transport.events) == before


@pytest.mark.parametrize(
    "field,value",
    [
        ("capabilities", {}),
        ("public_session", None),
        (
            "capabilities",
            dict(
                minimum_envelope_version=2, maximum_envelope_version=2, supports_cancellation=True
            ),
        ),
    ],
)
async def test_malformed_negotiation_closes_known_session(sandbox, field, value):
    sandbox._transport.open_data[field] = value
    with pytest.raises(ChannelError, match="negotiation"):
        async with sandbox.driver.connect():
            pass
    assert sandbox._transport.events[-1][1] == "DELETE"
    assert not sandbox.driver._connections


async def test_factory_failure_closes_opened_session(sandbox, native):
    def fail(channel):
        raise RuntimeError("factory failed")

    native.connect_remote_channel = fail
    with pytest.raises(RuntimeError, match="factory failed"):
        async with sandbox.driver.connect():
            pass
    assert sandbox._transport.events[-1][1] == "DELETE"


async def test_missing_completion_is_not_assumed_success(sandbox):
    async with sandbox.driver.connect() as driver:
        del sandbox._transport.response_data["completion_known"]
        with pytest.raises(ChannelError, match="malformed"):
            await driver.channel.exchange(envelope())


async def test_cancelled_request_never_dispatches_and_cancel_after_close_is_local(sandbox):
    async with sandbox.driver.connect() as driver:
        await driver.channel.cancel("request-1")
        with pytest.raises(ChannelError, match="cancelled"):
            await driver.channel.exchange(envelope())
        assert not any(
            isinstance(e, tuple) and e[2].endswith("exchange") for e in sandbox._transport.events
        )
    before = len(sandbox._transport.events)
    await driver.channel.cancel("request-2")
    assert len(sandbox._transport.events) == before


async def test_bind_session_is_unsupported_without_dispatch(sandbox):
    async with sandbox.driver.connect() as driver:
        with pytest.raises(ChannelError, match="rebinding is unsupported"):
            await driver.channel.bind_session(object())
        assert len(sandbox._transport.events) == 1


async def test_unsupported_transport_and_missing_service_do_not_load_native(monkeypatch):
    monkeypatch.setitem(sys.modules, "cua_driver", None)
    sb = Sandbox(LocalTransport(), _telemetry_enabled=False)
    with pytest.raises(DriverConnectionError, match="Fleet"):
        async with sb.driver.connect():
            pass
    transport = Transport()
    await transport.connect()
    transport._bound.services = ["server"]
    sb = Sandbox(transport, _telemetry_enabled=False)
    with pytest.raises(DriverConnectionError, match="expose"):
        async with sb.driver.connect():
            pass
    transport._bound.services.append("driver")
    with pytest.raises(DriverConnectionError, match="Install"):
        async with sb.driver.connect():
            pass
    assert not transport.events


def test_fleet_cloud_uses_supported_transport():
    assert issubclass(FleetCloudTransport, FleetTransport)


async def test_accessor_rejects_cleanup_from_another_event_loop(sandbox):
    async with sandbox.driver.connect():

        def wrong_loop():
            with pytest.raises(DriverConnectionError, match="different event loop"):
                asyncio.run(sandbox.driver.close())

        await asyncio.to_thread(wrong_loop)


async def test_context_cancellation_closes_session(sandbox):
    opened = asyncio.Event()

    async def run():
        async with sandbox.driver.connect():
            opened.set()
            await asyncio.Event().wait()

    task = asyncio.create_task(run())
    await opened.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert sandbox._transport.events[-1][1] == "DELETE"
    assert not sandbox.driver._connections


async def test_native_fleet_authorization_exception_is_sanitized(sandbox, monkeypatch):
    class Denied(Exception):
        status = 403

    async def denied(*args, **kwargs):
        raise Denied("synthetic-secret")

    monkeypatch.setattr(sandbox._transport, "request_service", denied)
    with pytest.raises(ChannelError, match="authorization denied") as error:
        async with sandbox.driver.connect():
            pass
    assert "synthetic-secret" not in str(error.value)
    assert error.value.__suppress_context__


async def test_late_response_after_cancel_is_not_returned(sandbox, monkeypatch):
    started, finish = asyncio.Event(), asyncio.Event()
    original = sandbox._transport.request_service

    async def delayed(*args, **kwargs):
        if kwargs["path"].endswith("/exchange"):
            started.set()
            await finish.wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", delayed)
    async with sandbox.driver.connect() as driver:
        task = asyncio.create_task(driver.channel.exchange(envelope()))
        await started.wait()
        await driver.channel.cancel("request-1")
        finish.set()
        with pytest.raises(ChannelError, match="completion is unknown"):
            await task


@pytest.mark.parametrize(
    "updates",
    [
        {"request_id": "wrong"},
        {"envelope_version": 2},
        {"completion_known": None},
    ],
)
async def test_invalid_response_immediately_invalidates_connection(sandbox, updates):
    async with sandbox.driver.connect() as driver:
        sandbox._transport.response_data.update(updates)
        with pytest.raises(ChannelError, match="malformed"):
            await driver.channel.exchange(envelope())
        assert driver.channel.closed
        assert sandbox._transport.events[-1][1] == "DELETE"
        before = len(sandbox._transport.events)
        with pytest.raises(ChannelError, match="closed"):
            await driver.channel.exchange(envelope(request_id="next"))
        assert len(sandbox._transport.events) == before


async def test_unknown_completion_preserves_envelope_and_invalidates(sandbox):
    async with sandbox.driver.connect() as driver:
        sandbox._transport.response_data.update(
            ok=False, completion_known=False, error="synthetic-secret", error_code="interrupted"
        )
        response = await driver.channel.exchange(envelope())
        assert response.completion_known is False
        assert response.error_code == "interrupted"
        assert response.error == "Driver operation failed"
        assert driver.channel.closed
        assert sandbox._transport.events[-1][1] == "DELETE"


async def test_session_name_only_for_active_owned_driver(sandbox):
    async with sandbox.driver.connect() as driver:
        assert sandbox.driver.session_name(driver) == "session-1"
        with pytest.raises(DriverConnectionError, match="another sandbox"):
            sandbox.driver.session_name(object())
    with pytest.raises(DriverConnectionError, match="inactive"):
        sandbox.driver.session_name(driver)


async def test_hanging_cleanup_does_not_block_claim_release(sandbox, monkeypatch, caplog):
    monkeypatch.setattr("cua_sandbox.interfaces.driver._CLEANUP_TIMEOUT", 0.01)
    original = sandbox._transport.request_service

    async def hanging_delete(*args, **kwargs):
        if kwargs["method"] == "DELETE":
            await asyncio.Event().wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", hanging_delete)

    async def release():
        sandbox._transport.events.append("release")

    sandbox._claim_handle = SimpleNamespace(release=release, name=None)
    async with sandbox.driver.connect() as driver:

        async def hanging_shutdown():
            await asyncio.Event().wait()

        driver.shutdown = hanging_shutdown
        await asyncio.wait_for(sandbox.close(), 0.5)
        assert driver.channel.closed
        assert not driver.channel.cleanup_confirmed
        assert sandbox._transport.events[-2:] == ["release", "disconnect"]
        assert "remote session cleanup is unconfirmed" in caplog.text


async def test_cancelled_exchange_invalidates_and_deletes(sandbox, monkeypatch):
    started = asyncio.Event()
    original = sandbox._transport.request_service

    async def hanging_exchange(*args, **kwargs):
        if kwargs["path"].endswith("/exchange"):
            started.set()
            await asyncio.Event().wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", hanging_exchange)
    async with sandbox.driver.connect() as driver:
        task = asyncio.create_task(driver.channel.exchange(envelope()))
        await started.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert driver.channel.closed
        assert driver.channel.cleanup_confirmed
        assert sandbox._transport.events[-1][1] == "DELETE"


@pytest.mark.parametrize("close_claim", [False, True])
async def test_pending_open_does_not_block_disconnect(sandbox, monkeypatch, close_claim):
    monkeypatch.setattr("cua_sandbox.interfaces.driver._CLEANUP_TIMEOUT", 0.01)
    started = asyncio.Event()
    original = sandbox._transport.request_service

    async def pending(*args, **kwargs):
        if kwargs["path"] == "/v1/connections":
            started.set()
            await asyncio.Event().wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", pending)
    if close_claim:

        async def release():
            sandbox._transport.events.append("release")

        sandbox._claim_handle = SimpleNamespace(release=release, name=None)
    opening = asyncio.create_task(sandbox.driver.connect().__aenter__())
    await started.wait()
    await asyncio.wait_for(sandbox.close() if close_claim else sandbox.disconnect(), 0.5)
    with pytest.raises(asyncio.CancelledError):
        await opening
    assert not sandbox.driver._connections
    assert sandbox._transport.events[-1] == "disconnect"
    if close_claim:
        assert sandbox._transport.events[-2] == "release"


@pytest.mark.parametrize("cancel_context", [False, True])
async def test_late_open_is_cleaned_without_resurrecting(sandbox, monkeypatch, cancel_context):
    monkeypatch.setattr("cua_sandbox.interfaces.driver._CLEANUP_TIMEOUT", 0.01)
    started, cancelled, finish, deleted = (asyncio.Event() for _ in range(4))
    original = sandbox._transport.request_service

    async def resistant(*args, **kwargs):
        if kwargs["path"] == "/v1/connections":
            started.set()
            try:
                await finish.wait()
            except asyncio.CancelledError:
                cancelled.set()
                await finish.wait()
        response = await original(*args, **kwargs)
        if kwargs["method"] == "DELETE":
            deleted.set()
        return response

    monkeypatch.setattr(sandbox._transport, "request_service", resistant)
    opening = asyncio.create_task(sandbox.driver.connect().__aenter__())
    await started.wait()
    if cancel_context:
        opening.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(opening, 0.5)
    else:
        await asyncio.wait_for(sandbox.driver.close(), 0.5)
    await cancelled.wait()
    assert not sandbox.driver._connections
    finish.set()
    await asyncio.wait_for(deleted.wait(), 0.5)
    if not cancel_context:
        with pytest.raises(DriverConnectionError, match="disconnected"):
            await opening
    assert [e[1] for e in sandbox._transport.events] == ["POST", "DELETE"]


@pytest.mark.parametrize("deadline", [1120000, 10**1000])
async def test_driver_deadline_overrides_short_transport_timeout(sandbox, monkeypatch, deadline):
    monkeypatch.setattr("cua_sandbox.interfaces.driver.time.time", lambda: 1000)
    original = sandbox._transport.request_service
    timeouts = []

    async def record(*args, **kwargs):
        timeouts.append(kwargs.get("timeout"))
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", record)
    async with sandbox.driver.connect() as driver:
        await driver.channel.exchange(envelope(deadline_unix_ms=deadline))
    assert timeouts == [None, 120, None]
    assert sandbox._transport._timeout == 30


@pytest.mark.parametrize("deadline", [0, 999999, 1000000, -1])
async def test_expired_or_invalid_deadline_never_dispatches(sandbox, monkeypatch, deadline):
    monkeypatch.setattr("cua_sandbox.interfaces.driver.time.time", lambda: 1000)
    async with sandbox.driver.connect() as driver:
        with pytest.raises(ChannelError, match="expired|malformed"):
            await driver.channel.exchange(envelope(deadline_unix_ms=deadline))
        assert len(sandbox._transport.events) == 1


async def test_local_deadline_cancels_then_deletes_without_replay(sandbox, monkeypatch):
    monkeypatch.setattr("cua_sandbox.interfaces.driver.time.time", lambda: 1000)
    original = sandbox._transport.request_service
    dispatched = []

    async def pending(*args, **kwargs):
        dispatched.append((kwargs["method"], kwargs["path"]))
        if kwargs["path"].endswith("/exchange"):
            await asyncio.Event().wait()
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", pending)
    async with sandbox.driver.connect() as driver:
        with pytest.raises(TimeoutError):
            await driver.channel.exchange(envelope(deadline_unix_ms=1000010))
        assert driver.channel.closed
        assert driver.channel.cleanup_confirmed
    assert dispatched == [
        ("POST", "/v1/connections"),
        ("POST", "/v1/connections/connection-1/exchange"),
        ("POST", "/v1/connections/connection-1/cancel"),
        ("DELETE", "/v1/connections/connection-1"),
    ]
    assert not sandbox.driver._closing


async def test_deadline_bounds_cancellation_resistant_exchange(sandbox, monkeypatch):
    monkeypatch.setattr("cua_sandbox.interfaces.driver.time.time", lambda: 1000)
    cancelled, finish, returned = (asyncio.Event() for _ in range(3))
    original = sandbox._transport.request_service

    async def resistant(*args, **kwargs):
        if kwargs["path"].endswith("/exchange"):
            try:
                await finish.wait()
            except asyncio.CancelledError:
                cancelled.set()
                await finish.wait()
            result = await original(*args, **kwargs)
            returned.set()
            return result
        return await original(*args, **kwargs)

    monkeypatch.setattr(sandbox._transport, "request_service", resistant)
    async with sandbox.driver.connect() as driver:
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(driver.channel.exchange(envelope(deadline_unix_ms=1000010)), 0.5)
        await cancelled.wait()
        assert driver.channel.closed
        assert driver.channel.cleanup_confirmed
        finish.set()
        await asyncio.wait_for(returned.wait(), 0.5)
        with pytest.raises(ChannelError, match="closed"):
            await driver.channel.exchange(envelope(request_id="next"))
    assert [event[1] for event in sandbox._transport.events] == ["POST", "POST", "DELETE", "POST"]
