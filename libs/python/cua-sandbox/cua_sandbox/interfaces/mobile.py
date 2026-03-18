"""Mobile interface — touch, gestures, and hardware keys via ADB, backed by a Transport."""

from __future__ import annotations

import asyncio

from cua_sandbox.transport.base import Transport


class Mobile:
    """Mobile (Android) touch and hardware-key control.

    All touch coordinates are in screen pixels. Methods delegate to the
    transport's ``send()`` with ``action="shell"`` running ``adb shell input …``
    commands under the hood.
    """

    def __init__(self, transport: Transport):
        self._t = transport

    async def _shell(self, command: str) -> None:
        await self._t.send("shell", command=command)

    # ── Touch ─────────────────────────────────────────────────────────────

    async def tap(self, x: int, y: int) -> None:
        await self._shell(f"input tap {x} {y}")

    async def long_press(self, x: int, y: int, duration_ms: int = 1000) -> None:
        await self._shell(f"input swipe {x} {y} {x} {y} {duration_ms}")

    async def double_tap(self, x: int, y: int, delay: float = 0.1) -> None:
        await self._shell(f"input tap {x} {y}")
        await asyncio.sleep(delay)
        await self._shell(f"input tap {x} {y}")

    async def type_text(self, text: str) -> None:
        encoded = text.replace(" ", "%s")
        await self._shell(f"input text '{encoded}'")

    # ── Gestures ──────────────────────────────────────────────────────────

    async def swipe(self, x1: int, y1: int, x2: int, y2: int, duration_ms: int = 300) -> None:
        await self._shell(f"input swipe {x1} {y1} {x2} {y2} {duration_ms}")

    async def scroll_up(self, x: int, y: int, distance: int = 600, duration_ms: int = 400) -> None:
        await self._shell(f"input swipe {x} {y + distance} {x} {y} {duration_ms}")

    async def scroll_down(
        self, x: int, y: int, distance: int = 600, duration_ms: int = 400
    ) -> None:
        await self._shell(f"input swipe {x} {y} {x} {y + distance} {duration_ms}")

    async def scroll_left(
        self, x: int, y: int, distance: int = 400, duration_ms: int = 300
    ) -> None:
        await self._shell(f"input swipe {x + distance} {y} {x} {y} {duration_ms}")

    async def scroll_right(
        self, x: int, y: int, distance: int = 400, duration_ms: int = 300
    ) -> None:
        await self._shell(f"input swipe {x} {y} {x + distance} {y} {duration_ms}")

    async def fling(self, x1: int, y1: int, x2: int, y2: int) -> None:
        await self.swipe(x1, y1, x2, y2, duration_ms=80)

    async def pinch_in(self, cx: int, cy: int, spread: int = 300, duration_ms: int = 400) -> None:
        await asyncio.gather(
            self._shell(f"input swipe {cx - spread} {cy} {cx} {cy} {duration_ms}"),
            self._shell(f"input swipe {cx + spread} {cy} {cx} {cy} {duration_ms}"),
        )

    async def pinch_out(self, cx: int, cy: int, spread: int = 300, duration_ms: int = 400) -> None:
        await asyncio.gather(
            self._shell(f"input swipe {cx} {cy} {cx - spread} {cy} {duration_ms}"),
            self._shell(f"input swipe {cx} {cy} {cx + spread} {cy} {duration_ms}"),
        )

    # ── Hardware keys ─────────────────────────────────────────────────────

    async def key(self, keycode: int) -> None:
        await self._shell(f"input keyevent {keycode}")

    async def home(self) -> None:
        await self.key(3)

    async def back(self) -> None:
        await self.key(4)

    async def recents(self) -> None:
        await self.key(187)

    async def power(self) -> None:
        await self.key(26)

    async def volume_up(self) -> None:
        await self.key(24)

    async def volume_down(self) -> None:
        await self.key(25)

    async def enter(self) -> None:
        await self.key(66)

    async def backspace(self) -> None:
        await self.key(67)

    # ── System ────────────────────────────────────────────────────────────

    async def wake(self) -> None:
        await self.key(224)

    async def notifications(self) -> None:
        await self._shell("service call statusbar 1")

    async def close_notifications(self) -> None:
        await self._shell("service call statusbar 2")
