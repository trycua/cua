"""EnvTransport port forwarding for url= sandboxes: the SDK handle is made
on demand, so ``sb.tunnel.forward`` works on a direct cua-spacesd URL
(the SDK carries it over the driver's /tunnel WebSocket). Fakes only."""

from __future__ import annotations

import pytest
from cua_sandbox.transport import env as env_module
from cua_sandbox.transport.env import EnvTransport


class _Forward:
    def __init__(self, local_addr):
        self._local_addr = local_addr
        self.closed = False

    def local_addr(self):
        return self._local_addr

    def url(self):
        return f"http://{self._local_addr}"

    async def close(self):
        self.closed = True


@pytest.mark.asyncio
async def test_url_sandbox_forwards_through_an_sdk_handle(monkeypatch):
    connects = []
    forwards = []

    class Handle:
        async def forward(self, port):
            forwards.append(port)
            return _Forward("127.0.0.1:40001")

    async def fake_connect_url(url, token):
        connects.append((url, token))
        return Handle()

    monkeypatch.setattr(env_module, "_connect_url", fake_connect_url)
    transport = EnvTransport(url="http://10.0.0.5:3211", token="tok")
    first = await transport.forward_tunnel(8080)
    second = await transport.forward_tunnel(9090)
    assert second.sandbox_port == 9090
    assert connects == [("http://10.0.0.5:3211", "tok")], "one handle per transport"
    assert forwards == [8080, 9090]
    assert (first.host, first.port, first.sandbox_port) == ("127.0.0.1", 40001, 8080)
    await transport.close_tunnel(first)
    await transport.disconnect()


@pytest.mark.asyncio
async def test_non_numeric_ports_are_rejected_before_connecting(monkeypatch):
    async def fail(*_):
        raise AssertionError("must not connect")

    monkeypatch.setattr(env_module, "_connect_url", fail)
    with pytest.raises(ValueError, match="numeric"):
        await EnvTransport(url="http://10.0.0.5:3211").forward_tunnel("abstract")
