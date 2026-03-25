"""Tunnel interface — forward a port from the sandbox to the host.

Usage::

    # Async context manager (port released on exit)
    async with sb.tunnel.forward(9222) as t:
        print(t.url)          # "http://localhost:49823"
        print(t.host, t.port) # "localhost", 49823

    # Plain await (caller must call .close())
    t = await sb.tunnel.forward(9222)
    await t.close()

    # Multiple ports — returns dict[sandbox_port, TunnelInfo]
    async with sb.tunnel.forward(9222, 8080) as tunnels:
        devtools = tunnels[9222].url
        app      = tunnels[8080].url
"""

from __future__ import annotations

import asyncio
from typing import Dict, List, Optional, Union

from cua_sandbox.transport.base import Transport


class TunnelInfo:
    """A single forwarded port."""

    def __init__(self, host: str, port: int, sandbox_port: int) -> None:
        self.host = host
        self.port = port  # host-side port
        self.sandbox_port = sandbox_port  # original port inside sandbox
        self._closer: Optional[object] = None  # Callable[[TunnelInfo], Coroutine]

    @property
    def url(self) -> str:
        return f"http://{self.host}:{self.port}"

    async def close(self) -> None:
        """Close this tunnel (no-op if already closed or inside a context manager)."""
        if self._closer is not None:
            await self._closer(self)  # type: ignore[operator]
            self._closer = None

    def __repr__(self) -> str:
        return f"TunnelInfo(host={self.host!r}, port={self.port}, sandbox_port={self.sandbox_port})"


class _TunnelContext:
    """Returned by Tunnel.forward() — supports both await and async with."""

    def __init__(self, transport: Transport, ports: tuple[int, ...]):
        self._t = transport
        self._ports = ports
        self._infos: List[TunnelInfo] = []

    # ── awaitable ─────────────────────────────────────────────────────────────

    def __await__(self):
        return self._open().__await__()

    async def _open(self) -> Union[TunnelInfo, Dict[int, TunnelInfo]]:
        for p in self._ports:
            info = await self._t.forward_tunnel(p)
            info._closer = self._close_one
            self._infos.append(info)
        return self._result()

    def _result(self) -> Union[TunnelInfo, Dict[int, TunnelInfo]]:
        if len(self._infos) == 1:
            return self._infos[0]
        return {i.sandbox_port: i for i in self._infos}

    # ── async context manager ─────────────────────────────────────────────────

    async def __aenter__(self) -> Union[TunnelInfo, Dict[int, TunnelInfo]]:
        return await self._open()

    async def __aexit__(self, *_) -> None:
        await asyncio.gather(*(self._t.close_tunnel(i) for i in self._infos))
        self._infos.clear()

    async def _close_one(self, info: TunnelInfo) -> None:
        await self._t.close_tunnel(info)
        self._infos = [i for i in self._infos if i is not info]


class Tunnel:
    """Port-forwarding interface — exposes sandbox ports on the host."""

    def __init__(self, transport: Transport):
        self._t = transport

    def forward(self, *ports: int) -> _TunnelContext:
        """Forward one or more sandbox ports to the host.

        Returns a context manager (or awaitable) that yields:
        - a single :class:`TunnelInfo` when one port is given
        - a ``dict[sandbox_port, TunnelInfo]`` when multiple ports are given
        """
        if not ports:
            raise ValueError("forward() requires at least one port")
        return _TunnelContext(self._t, ports)
