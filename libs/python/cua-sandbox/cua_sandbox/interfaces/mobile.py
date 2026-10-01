"""Mobile interface — touch, gestures, and hardware keys via ADB, backed by a Transport."""

from __future__ import annotations

import asyncio

from cua_sandbox.transport.base import Transport


class Mobile:
    """Mobile (Android) touch and hardware-key control.

    All touch coordinates are in screen pixels.  Single-touch methods use
    ``input tap/swipe`` via ``adb shell``.  Multi-touch gestures delegate to
    the ``multitouch_gesture`` transport action, which uses ``adb root`` +
    MT Protocol B ``sendevent`` for reliable injection on both local and cloud
    transports.
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

    async def _get_screen_size(self) -> tuple[int, int]:
        """Return ``(width, height)`` in screen pixels via ``wm size``."""
        result = await self._t.send("shell", command="wm size")
        output = result.get("stdout", "").strip()
        for line in output.splitlines():
            if "size" in line.lower():
                dims = line.split(":")[-1].strip()
                w, h = dims.split("x")
                return int(w), int(h)
        return 32768, 32768  # fallback: 1:1 mapping

    async def gesture(
        self,
        *finger_paths: tuple[int, int],
        duration_ms: int = 400,
        steps: int = 0,
    ) -> None:
        """Inject an arbitrary N-finger gesture via MT Protocol B ``sendevent``.

        Each positional argument is a sequence of ``(x, y)`` waypoints for one
        finger.  Pass an even number of ``(x, y)`` tuples and they are paired
        into start/end positions per finger::

            # two-finger pinch-out
            await mobile.gesture(
                (cx - 20, cy), (cx - 200, cy),   # finger 0
                (cx + 20, cy), (cx + 200, cy),   # finger 1
            )

        Delegates to the ``multitouch_gesture`` transport action, which uses
        ``adb root`` + MT Protocol B ``sendevent`` for reliable injection on
        both local ADB and cloud HTTP transports.

        Args:
            *finger_paths: Alternating start/end ``(x, y)`` tuples, two per finger.
                           Must have an even length >= 4 (at least 2 fingers × 2 points).
            duration_ms:   Total gesture duration in milliseconds.
            steps:         Interpolation steps (0 = auto: duration_ms // 20, min 5).
        """
        if len(finger_paths) < 4 or len(finger_paths) % 2 != 0:
            raise ValueError(
                "gesture() requires an even number of (x, y) tuples, two per finger "
                f"(start + end).  Got {len(finger_paths)} tuples."
            )

        # Pair waypoints into per-finger (start, end) tuples
        fingers_pairs: list[tuple[tuple[int, int], tuple[int, int]]] = []
        for i in range(0, len(finger_paths), 2):
            fingers_pairs.append((finger_paths[i], finger_paths[i + 1]))

        sw, sh = await self._get_screen_size()
        n_steps = steps if steps > 0 else max(5, duration_ms // 20)

        # Structured params for the server-side multitouch_gesture action
        fingers_payload = [{"start": list(start), "end": list(end)} for start, end in fingers_pairs]
        await self._t.send(
            "multitouch_gesture",
            fingers=fingers_payload,
            screen_w=sw,
            screen_h=sh,
            duration_ms=duration_ms,
            steps=n_steps,
        )

    async def pinch_in(self, cx: int, cy: int, spread: int = 300, duration_ms: int = 400) -> None:
        """Pinch-in (zoom out) with two real simultaneous fingers."""
        await self.gesture(
            (cx - spread, cy),
            (cx - 20, cy),
            (cx + spread, cy),
            (cx + 20, cy),
            duration_ms=duration_ms,
        )

    async def pinch_out(self, cx: int, cy: int, spread: int = 300, duration_ms: int = 400) -> None:
        """Pinch-out (zoom in) with two real simultaneous fingers."""
        await self.gesture(
            (cx - 20, cy),
            (cx - spread, cy),
            (cx + 20, cy),
            (cx + spread, cy),
            duration_ms=duration_ms,
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
