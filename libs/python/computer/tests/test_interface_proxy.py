"""Unit tests for direct-proxy support in the computer interface.

These tests verify that passing an explicit ``api_base_url`` (plus arbitrary
``api_headers``) makes the interface derive its REST/WebSocket/Playwright URLs
from that base URL -- preserving scheme, host, port and path prefix -- and sends
the extra headers. This is what allows connecting directly to a computer-server
behind an authenticated, path-prefixed reverse proxy (e.g. cyclops-cs) without a
localhost forwarder.

``api_headers`` may also be a callable, sync or async, that is re-resolved on
every connection attempt and every REST request. A proxy that authenticates with
a short-lived bearer (Fleet mints 900-second Keycloak tokens) needs that: a dict
captured once goes stale mid-session and the reconnect that follows replays a
dead token. Those tests live here too, since this is header propagation.

Following SRP: This file tests ONLY URL building and header propagation for the
interface layer.
"""

import asyncio

import pytest
import websockets
from computer.interface.factory import InterfaceFactory
from computer.interface.windows import WindowsComputerInterface

PROXY_BASE = "https://run.cua.ai/api/svc/ns/sb-api"


class TestApiBaseUrlBuilding:
    """api_base_url should fully drive REST/WS/Playwright URLs."""

    def test_rest_uri_uses_base_url(self):
        iface = WindowsComputerInterface(
            "ignored-ip", api_base_url=PROXY_BASE, api_headers={"Authorization": "Bearer t"}
        )
        assert iface.rest_uri == f"{PROXY_BASE}/cmd"

    def test_ws_uri_swaps_scheme_and_keeps_prefix(self):
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE)
        assert iface.ws_uri == "wss://run.cua.ai/api/svc/ns/sb-api/ws"

    def test_trailing_slash_is_stripped(self):
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE + "/")
        assert iface.rest_uri == f"{PROXY_BASE}/cmd"
        assert iface.ws_uri == "wss://run.cua.ai/api/svc/ns/sb-api/ws"

    def test_http_base_maps_to_ws(self):
        iface = WindowsComputerInterface("ignored-ip", api_base_url="http://localhost:9999/prefix")
        assert iface.rest_uri == "http://localhost:9999/prefix/cmd"
        assert iface.ws_uri == "ws://localhost:9999/prefix/ws"

    def test_api_headers_stored(self):
        headers = {"Authorization": "Bearer secret"}
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE, api_headers=headers)
        assert iface._api_headers == headers

    def test_factory_threads_params(self):
        iface = InterfaceFactory.create_interface_for_os(
            os="windows",
            ip_address="ignored-ip",
            api_base_url=PROXY_BASE,
            api_headers={"Authorization": "Bearer t"},
        )
        assert iface.rest_uri == f"{PROXY_BASE}/cmd"
        assert iface.ws_uri == "wss://run.cua.ai/api/svc/ns/sb-api/ws"
        assert iface._api_headers == {"Authorization": "Bearer t"}


class TestBackwardCompatibility:
    """Without api_base_url, behavior is unchanged (ip_address + port)."""

    def test_no_base_url_local(self):
        iface = WindowsComputerInterface("192.168.1.5")
        assert iface.rest_uri == "http://192.168.1.5:8000/cmd"
        assert iface.ws_uri == "ws://192.168.1.5:8000/ws"
        assert iface._api_headers == {}

    def test_no_base_url_with_api_key(self):
        iface = WindowsComputerInterface("192.168.1.5", api_key="abc", vm_name="vm")
        assert iface.rest_uri == "https://192.168.1.5:8443/cmd"
        assert iface.ws_uri == "wss://192.168.1.5:8443/ws"

    def test_no_base_url_custom_port(self):
        iface = WindowsComputerInterface("192.168.1.5", api_port=1234)
        assert iface.rest_uri == "http://192.168.1.5:1234/cmd"
        assert iface.ws_uri == "ws://192.168.1.5:1234/ws"


class TestRestHeaderMerging:
    """api_headers must be merged into REST requests."""

    @pytest.mark.asyncio
    async def test_send_command_rest_sends_headers(self, monkeypatch):
        captured = {}

        class FakeResp:
            status = 200

            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            async def text(self):
                return 'data: {"success": true}'

        class FakeSession:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *a):
                return False

            def post(self, url, json=None, headers=None):
                captured["url"] = url
                captured["headers"] = headers
                return FakeResp()

        import aiohttp

        monkeypatch.setattr(aiohttp, "ClientSession", lambda *a, **k: FakeSession())

        iface = WindowsComputerInterface(
            "ignored-ip",
            api_base_url=PROXY_BASE,
            api_headers={"Authorization": "Bearer secret"},
        )
        result = await iface._send_command_rest("version", {})
        assert result.get("success") is True
        assert captured["url"] == f"{PROXY_BASE}/cmd"
        assert captured["headers"]["Authorization"] == "Bearer secret"


# ---------------------------------------------------------------------------
# Re-resolvable header sources
# ---------------------------------------------------------------------------


class SyncTokenSource:
    """Mints ``jwt-N`` per call, one number per resolution.

    Mirrors ``FakeTokenEndpoint`` in ``test_fleet_provider.py``: a source whose
    value changes on every call is the only way to tell a header that was
    resolved once from one that is resolved per attempt.
    """

    def __init__(self) -> None:
        self.calls = 0

    def _mint(self):
        self.calls += 1
        return {"Authorization": f"Bearer jwt-{self.calls}"}

    def __call__(self):
        return self._mint()


class AsyncTokenSource(SyncTokenSource):
    """The Fleet shape: resolving the bearer is a coroutine, not a lookup."""

    async def __call__(self):
        return self._mint()


class FakeWebSocket:
    """Enough of a websockets connection for ``_keep_alive`` to hold it open."""

    def __init__(self) -> None:
        self.state = websockets.protocol.State.OPEN

    async def ping(self):
        pong = asyncio.get_running_loop().create_future()
        pong.set_result(None)
        return pong

    async def close(self):
        self.state = websockets.protocol.State.CLOSED


async def _wait_until(predicate, message, timeout: float = 5.0):
    """Poll ``predicate`` until true, failing rather than hanging on a bug."""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while loop.time() < deadline:
        if predicate():
            return
        await asyncio.sleep(0.01)
    raise AssertionError(f"timed out waiting for {message}")


async def _drive_two_connection_attempts(iface, attempts):
    """Run the interface's reconnect loop across one drop and back.

    Drives the real ``_keep_alive`` rather than calling a helper directly, so
    the test fails if the reconnect path stops consulting the header source --
    which is exactly the bug being fixed.
    """
    task = asyncio.create_task(iface._keep_alive())
    try:
        await _wait_until(lambda: len(attempts) >= 1, "the first connection attempt")
        # What a network blip looks like from here: the socket is closed and
        # the keep-alive loop dials again.
        iface._ws.state = websockets.protocol.State.CLOSED
        await _wait_until(lambda: len(attempts) >= 2, "the reconnect attempt")
    finally:
        iface._closed = True
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


@pytest.fixture
def ws_attempts(monkeypatch):
    """Capture the upgrade headers of every WebSocket connection attempt."""
    attempts = []

    async def fake_connect(uri, **kwargs):
        headers = kwargs.get("additional_headers") or kwargs.get("extra_headers") or []
        attempts.append(dict(headers))
        return FakeWebSocket()

    monkeypatch.setattr(websockets, "connect", fake_connect)
    return attempts


@pytest.fixture
def rest_requests(monkeypatch):
    """Capture the headers of every REST request the interface sends."""
    requests = []

    class FakeResp:
        status = 200

        async def __aenter__(self):
            return self

        async def __aexit__(self, *a):
            return False

        async def text(self):
            return 'data: {"success": true}'

        async def json(self):
            return {"success": True}

    class FakeSession:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *a):
            return False

        def post(self, url, json=None, headers=None):
            requests.append(dict(headers or {}))
            return FakeResp()

    import aiohttp

    monkeypatch.setattr(aiohttp, "ClientSession", lambda *a, **k: FakeSession())
    return requests


class TestReResolvableApiHeaders:
    """A callable ``api_headers`` is resolved per attempt, a dict is not.

    Fleet's bearer is a Keycloak access token that lives 900 seconds while a
    bench run lives hours. The open socket survives expiry -- the proxy
    authenticates the upgrade, not each frame -- so the failure only shows up
    on the first reconnect after the token lapses, and then permanently.
    """

    @pytest.mark.asyncio
    async def test_reconnect_re_resolves_an_async_callable_source(self, ws_attempts):
        """The headline: the second upgrade must carry the second token."""
        source = AsyncTokenSource()
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE, api_headers=source)

        await _drive_two_connection_attempts(iface, ws_attempts)

        assert ws_attempts[:2] == [
            {"Authorization": "Bearer jwt-1"},
            {"Authorization": "Bearer jwt-2"},
        ]

    @pytest.mark.asyncio
    async def test_a_dict_source_is_never_re_resolved(self, ws_attempts):
        """CLOUD/CLOUDV2 pass a dict; it must behave exactly as before.

        Mutating the caller's dict after construction proves the point twice:
        the interface still holds its own copy (today's behaviour) and it does
        not go back to any source between attempts.
        """
        headers = {"Authorization": "Bearer static"}
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE, api_headers=headers)
        headers["Authorization"] = "Bearer mutated-after-construction"

        await _drive_two_connection_attempts(iface, ws_attempts)

        assert ws_attempts[:2] == [
            {"Authorization": "Bearer static"},
            {"Authorization": "Bearer static"},
        ]
        assert iface._api_headers == {"Authorization": "Bearer static"}

    @pytest.mark.asyncio
    async def test_a_callable_source_is_not_resolved_at_construction(self):
        """Construction is sync, so an async source cannot be awaited there.

        Resolving lazily is what lets the source be a coroutine function at
        all; a source called in ``__init__`` would have to be sync forever.
        """
        source = AsyncTokenSource()
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE, api_headers=source)

        assert source.calls == 0
        assert iface._api_headers == {}

    @pytest.mark.asyncio
    async def test_rest_requests_re_resolve_a_sync_callable_source(self, rest_requests):
        """REST rides the same expiring bearer as the socket.

        A sync callable covers a caller whose token lives in memory and needs
        no I/O to refresh.
        """
        source = SyncTokenSource()
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE, api_headers=source)

        await iface._send_command_rest("version", {})
        await iface._send_command_rest("version", {})

        assert [r["Authorization"] for r in rest_requests] == [
            "Bearer jwt-1",
            "Bearer jwt-2",
        ]

    @pytest.mark.asyncio
    async def test_playwright_exec_re_resolves_an_async_callable_source(self, rest_requests):
        """The Playwright endpoint is a third caller of the same headers."""
        source = AsyncTokenSource()
        iface = WindowsComputerInterface("ignored-ip", api_base_url=PROXY_BASE, api_headers=source)

        await iface.playwright_exec("visit_url", {"url": "https://example.com"})
        await iface.playwright_exec("visit_url", {"url": "https://example.com"})

        assert [r["Authorization"] for r in rest_requests] == [
            "Bearer jwt-1",
            "Bearer jwt-2",
        ]
