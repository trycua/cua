"""Unit tests for direct-proxy support in the computer interface.

These tests verify that passing an explicit ``api_base_url`` (plus arbitrary
``api_headers``) makes the interface derive its REST/WebSocket/Playwright URLs
from that base URL -- preserving scheme, host, port and path prefix -- and sends
the extra headers. This is what allows connecting directly to a computer-server
behind an authenticated, path-prefixed reverse proxy (e.g. cyclops-cs) without a
localhost forwarder.

Following SRP: This file tests ONLY URL building and header propagation for the
interface layer.
"""

import pytest
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
