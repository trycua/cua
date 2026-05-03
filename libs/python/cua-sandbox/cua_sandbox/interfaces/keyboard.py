"""Keyboard interface — type text and press keys, backed by a Transport."""

from __future__ import annotations

from typing import List, Union

from cua_sandbox.transport.base import Transport


class Keyboard:
    """Keyboard control."""

    def __init__(self, transport: Transport):
        self._t = transport

    async def type(self, text: str) -> None:
        """Type a string of text."""
        await self._t.send("type_text", text=text)

    async def keypress(self, keys: Union[List[str], str]) -> None:
        """Press a key combination (e.g. ["ctrl", "c"] or "enter")."""
        if isinstance(keys, str):
            keys = [keys]
        await self._t.send("hotkey", keys=keys)

    async def key_down(self, key: str) -> None:
        await self._t.send("key_down", key=key)

    async def key_up(self, key: str) -> None:
        await self._t.send("key_up", key=key)
