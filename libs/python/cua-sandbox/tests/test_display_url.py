"""``Sandbox.get_display_url()`` on SDK-backed sandboxes: the cua-spacesd
viewer link when the sandbox runs cua-spacesd, else the page of a declared
legacy web display service (local loopback or a Fleet signed URL). Fakes only;
nothing is started."""

from __future__ import annotations

from types import SimpleNamespace

import pytest
from cua_sandbox._sdk import native
from cua_sandbox.transport.env import (
    EnvTransport,
    display_service_name,
    novnc_page_url,
)


class _Service:
    def __init__(self, url: str) -> None:
        self._url = url
        self.public_calls = 0

    async def url(self) -> str:
        return self._url

    async def public_url(self, ttl_seconds=None, label=None):
        self.public_calls += 1
        return SimpleNamespace(url=self._url.replace("127.0.0.1", "share.example") + "?t=abc")


VIEWER = "http://127.0.0.1:40999/viewer/#ticket=abc"


class _Handle:
    def __init__(self, services: dict, urls: dict, viewer=None) -> None:
        self._services = services
        self._urls = urls
        self._viewer = viewer
        self.asked: list[str] = []
        self.viewer_calls = 0

    async def viewer_url(self, options=None):
        self.viewer_calls += 1
        if isinstance(self._viewer, BaseException):
            raise self._viewer
        if self._viewer is None:
            raise AssertionError("viewer_url must not be called")
        return SimpleNamespace(url=self._viewer, expires_at_unix=1_900_000_000)

    def info(self):
        return SimpleNamespace(services=self._services, endpoints={})

    def service(self, name: str):
        self.asked.append(name)
        return _Service(self._urls[name])


async def _never():
    raise AssertionError("the display URL must not open cua-spacesd")


def test_display_service_preference_and_raw_rfb_is_skipped():
    assert display_service_name({"env": 3211, "novnc": 6080, "web": 8080}) == "novnc"
    assert display_service_name({"vnc": 6080}) is None
    assert display_service_name({"novnc": 5901}) is None
    assert display_service_name({"novnc": 5901, "web": 8080}) == "web"
    assert display_service_name({"display": None}) == "display"
    assert display_service_name({"env": 3211}) is None


def test_novnc_page_url_shapes():
    assert (
        novnc_page_url("http://127.0.0.1:40123")
        == "http://127.0.0.1:40123/vnc.html?autoconnect=1&resize=scale"
    )
    # A gateway route keeps its prefix and the websocket follows it.
    routed = novnc_page_url("https://gw.example/api/svc/ns/box/novnc/?sig=xyz")
    assert routed.startswith("https://gw.example/api/svc/ns/box/novnc/vnc.html?sig=xyz&")
    assert "autoconnect=1" in routed and "resize=scale" in routed
    assert "path=api%2Fsvc%2Fns%2Fbox%2Fnovnc%2Fwebsockify%3Fsig%3Dxyz" in routed
    # Already a page: unchanged.
    page = "http://127.0.0.1:1/vnc.html?autoconnect=1"
    assert novnc_page_url(page) == page


@pytest.mark.asyncio
@pytest.mark.parametrize("share", [False, True])
async def test_spacesd_sandbox_returns_the_viewer_link(share):
    handle = _Handle({"env": 3211}, {}, viewer=VIEWER)
    transport = EnvTransport(
        env_factory=_never, native_sandbox=handle, vnc_url="vnc://127.0.0.1:5901"
    )
    assert await transport.get_display_url(share=share) == VIEWER
    assert handle.viewer_calls == 1 and handle.asked == []


@pytest.mark.asyncio
async def test_viewer_wins_over_a_legacy_display_service():
    handle = _Handle({"env": 3211, "novnc": 6080}, {"novnc": "http://x"}, viewer=VIEWER)
    transport = EnvTransport(env_factory=_never, native_sandbox=handle)
    assert await transport.get_display_url() == VIEWER
    assert handle.asked == []


@pytest.mark.asyncio
async def test_no_spacesd_falls_back_to_the_legacy_display_service():
    handle = _Handle(
        {"env": 3211, "novnc": 6080},
        {"novnc": "http://127.0.0.1:40123"},
        viewer=native().CuaError.SpacesdNotAvailable("no spacesd"),
    )
    transport = EnvTransport(
        env_factory=_never, native_sandbox=handle, vnc_url="vnc://127.0.0.1:5901"
    )
    url = await transport.get_display_url()
    assert url == "http://127.0.0.1:40123/vnc.html?autoconnect=1&resize=scale"
    assert handle.asked == ["novnc"]


@pytest.mark.asyncio
async def test_viewer_errors_other_than_no_spacesd_propagate():
    handle = _Handle({"env": 3211}, {}, viewer=RuntimeError("boom"))
    transport = EnvTransport(env_factory=_never, native_sandbox=handle)
    with pytest.raises(RuntimeError, match="boom"):
        await transport.get_display_url()


@pytest.mark.asyncio
async def test_legacy_image_share_uses_the_public_url():
    handle = _Handle({"novnc": 6080}, {"novnc": "http://127.0.0.1:40123"})
    transport = EnvTransport(env_factory=_never, native_sandbox=handle)
    url = await transport.get_display_url(share=True)
    assert url.startswith("http://share.example:40123/vnc.html?t=abc&autoconnect=1")


@pytest.mark.asyncio
async def test_without_spacesd_or_a_display_service_the_vnc_address_is_used():
    handle = _Handle({"ssh": 22}, {})
    transport = EnvTransport(
        env_factory=_never, native_sandbox=handle, vnc_url="vnc://127.0.0.1:5901"
    )
    assert await transport.get_display_url() == "vnc://127.0.0.1:5901"


@pytest.mark.asyncio
async def test_no_display_points_at_cua_spacesd():
    transport = EnvTransport(env_factory=_never, native_sandbox=_Handle({"ssh": 22}, {}))
    with pytest.raises(NotImplementedError) as info:
        await transport.get_display_url()
    message = str(info.value)
    assert "cua-spacesd" in message and "3211" in message
    assert "novnc" not in message.lower() and "6080" not in message
    assert "EnvTransport" not in message


@pytest.mark.asyncio
async def test_fleet_transport_returns_the_viewer_link(monkeypatch):
    from cua_sandbox.transport import fleet as fleet_module

    handle = _Handle({}, {}, viewer="https://gw.example/api/svc/ns/box/env/viewer/#t=1")

    async def fake_fleet_sandbox(*_args):
        return handle

    monkeypatch.setattr(fleet_module, "fleet_sandbox", fake_fleet_sandbox)
    bound = SimpleNamespace(name="box", namespace="ns", claim="c1", services={"env": 3211})
    transport = fleet_module.FleetTransport(sdk=object(), bound=bound)
    assert await transport.get_display_url() == handle._viewer


@pytest.mark.asyncio
async def test_fleet_transport_reads_the_claims_legacy_services(monkeypatch):
    from cua_sandbox.transport import fleet as fleet_module

    handle = _Handle({}, {"novnc": "https://gw.example/api/svc/ns/box/novnc?sig=1"})

    async def fake_fleet_sandbox(*_args):
        return handle

    monkeypatch.setattr(fleet_module, "fleet_sandbox", fake_fleet_sandbox)
    bound = SimpleNamespace(name="box", namespace="ns", claim="c1", services={"novnc": 6080})
    transport = fleet_module.FleetTransport(sdk=object(), bound=bound)
    url = await transport.get_display_url()
    assert url.startswith("https://gw.example/api/svc/ns/box/novnc/vnc.html?sig=1&autoconnect=1")
