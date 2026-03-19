"""Mobile interface — touch, gestures, and hardware keys via ADB, backed by a Transport."""

from __future__ import annotations

import asyncio

from cua_sandbox.transport.base import Transport

# ── MT Protocol B constants (Linux kernel event codes, decimal) ────────────
_EV_SYN = 0
_EV_KEY = 1
_EV_ABS = 3
_SYN_REPORT = 0
_BTN_TOUCH = 330
_ABS_MT_SLOT = 47
_ABS_MT_TID = 57  # ABS_MT_TRACKING_ID
_ABS_MT_X = 53  # ABS_MT_POSITION_X
_ABS_MT_Y = 54  # ABS_MT_POSITION_Y
_ABS_MT_PRESS = 58  # ABS_MT_PRESSURE
_TID_NONE = 4294967295  # tracking-id -1 (unsigned) → finger lift


_AXIS_MAX = 32767  # raw axis range for virtio/goldfish touch devices


def _se(dev: str, etype: int, code: int, value: int) -> str:
    """One sendevent command fragment, run as root so the shell user can write to /dev/input/*."""
    return f"su root sendevent {dev} {etype} {code} {value}"


def _sync(dev: str) -> str:
    return _se(dev, _EV_SYN, _SYN_REPORT, 0)


def _px_to_raw(px: int, screen_dim: int) -> int:
    """Scale a screen-pixel coordinate to the 0-32767 raw axis range."""
    return max(0, min(_AXIS_MAX, int(px * _AXIS_MAX / screen_dim)))


def _build_two_finger_script(
    dev: str,
    x1_start: int,
    y1_start: int,
    x2_start: int,
    y2_start: int,
    x1_end: int,
    y1_end: int,
    x2_end: int,
    y2_end: int,
    steps: int,
    step_delay_s: float,
    screen_w: int = _AXIS_MAX + 1,
    screen_h: int = _AXIS_MAX + 1,
) -> str:
    """
    Build a single shell script (commands joined with ``&&``) that injects a
    two-finger gesture using raw Linux MT Protocol B ``sendevent`` calls.

    Every step is a single ``SYN_REPORT`` frame containing both finger
    positions, so Android sees genuine ``pointer_count == 2`` events.  Sending
    the whole sequence as **one** shell invocation means all timing happens
    on-device — no per-step ADB round-trips.

    All x/y arguments are in screen pixels.  ``screen_w`` and ``screen_h`` are
    used to scale them to the device's raw 0-32767 axis range.
    """

    def rx(px: int) -> int:
        return _px_to_raw(px, screen_w)

    def ry(px: int) -> int:
        return _px_to_raw(px, screen_h)

    cmds: list[str] = []

    # Touch down — both fingers in one sync frame
    cmds += [
        _se(dev, _EV_ABS, _ABS_MT_SLOT, 0),
        _se(dev, _EV_ABS, _ABS_MT_TID, 0),
        _se(dev, _EV_ABS, _ABS_MT_X, rx(x1_start)),
        _se(dev, _EV_ABS, _ABS_MT_Y, ry(y1_start)),
        _se(dev, _EV_ABS, _ABS_MT_PRESS, 64),
        _se(dev, _EV_ABS, _ABS_MT_SLOT, 1),
        _se(dev, _EV_ABS, _ABS_MT_TID, 1),
        _se(dev, _EV_ABS, _ABS_MT_X, rx(x2_start)),
        _se(dev, _EV_ABS, _ABS_MT_Y, ry(y2_start)),
        _se(dev, _EV_ABS, _ABS_MT_PRESS, 64),
        _se(dev, _EV_KEY, _BTN_TOUCH, 1),
        _sync(dev),
    ]

    # Interpolated movement
    for i in range(1, steps + 1):
        t = i / steps
        cmds += [
            _se(dev, _EV_ABS, _ABS_MT_SLOT, 0),
            _se(dev, _EV_ABS, _ABS_MT_X, rx(int(x1_start + (x1_end - x1_start) * t))),
            _se(dev, _EV_ABS, _ABS_MT_Y, ry(int(y1_start + (y1_end - y1_start) * t))),
            _se(dev, _EV_ABS, _ABS_MT_SLOT, 1),
            _se(dev, _EV_ABS, _ABS_MT_X, rx(int(x2_start + (x2_end - x2_start) * t))),
            _se(dev, _EV_ABS, _ABS_MT_Y, ry(int(y2_start + (y2_end - y2_start) * t))),
            _sync(dev),
            f"sleep {step_delay_s:.3f}",
        ]

    # Touch up — both fingers in one sync frame
    cmds += [
        _se(dev, _EV_ABS, _ABS_MT_SLOT, 0),
        _se(dev, _EV_ABS, _ABS_MT_TID, _TID_NONE),
        _se(dev, _EV_ABS, _ABS_MT_SLOT, 1),
        _se(dev, _EV_ABS, _ABS_MT_TID, _TID_NONE),
        _se(dev, _EV_KEY, _BTN_TOUCH, 0),
        _sync(dev),
    ]

    return " && ".join(cmds)


class Mobile:
    """Mobile (Android) touch and hardware-key control.

    All touch coordinates are in screen pixels.  Methods delegate to the
    transport's ``send()`` with ``action="shell"`` running ``adb shell …``
    commands under the hood.

    **Multi-touch (pinch_in / pinch_out)**

    True multi-touch requires both fingers to appear in the **same**
    ``SYN_REPORT`` frame delivered to the kernel.  The previous implementation
    used ``asyncio.gather`` over two separate ``adb shell input swipe`` calls,
    which creates two independent single-touch streams — the device never sees
    ``pointer_count > 1``.

    The corrected implementation builds a single shell script of chained
    ``sendevent`` commands (joined with ``&&``) so the entire gesture — touch
    down, interpolated movement, and touch up — executes on-device in one
    ``adb shell`` invocation with no Python round-trips between steps.
    """

    def __init__(self, transport: Transport):
        self._t = transport
        self._touch_dev: str | None = None  # cached event device path

    async def _shell(self, command: str) -> None:
        await self._t.send("shell", command=command)

    async def _find_touch_device(self) -> str:
        """Return the ``/dev/input/eventX`` path for the primary touchscreen."""
        if self._touch_dev is not None:
            return self._touch_dev
        result = await self._t.send(
            "shell",
            command=(
                "for f in /dev/input/event*; do "
                "  getevent -p \"$f\" 2>/dev/null | grep -q '0035' "
                '    && echo "$f" && break; '
                "done"
            ),
        )
        dev = result.get("stdout", "").strip()
        self._touch_dev = dev if dev else "/dev/input/event1"
        return self._touch_dev

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
        return _AXIS_MAX + 1, _AXIS_MAX + 1  # fallback: 1:1 mapping

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

        The entire gesture executes in a single ``adb shell`` invocation so
        all timing is on-device.

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
        fingers: list[tuple[tuple[int, int], tuple[int, int]]] = []
        for i in range(0, len(finger_paths), 2):
            fingers.append((finger_paths[i], finger_paths[i + 1]))

        dev = await self._find_touch_device()
        sw, sh = await self._get_screen_size()
        n_steps = steps if steps > 0 else max(5, duration_ms // 20)
        step_delay = (duration_ms / 1000) / n_steps

        EV_KEY, EV_ABS = 1, 3
        BTN_TOUCH = 330
        SLOT, TID, MTX, MTY, PRESS = 47, 57, 53, 54, 58
        TID_NONE = 4294967295

        def rx(px: int) -> int:
            return _px_to_raw(px, sw)

        def ry(px: int) -> int:
            return _px_to_raw(px, sh)

        def se(t: int, c: int, v: int) -> str:
            return _se(dev, t, c, v)

        def sync() -> str:
            return _sync(dev)

        cmds: list[str] = []

        # Touch down — all fingers in one sync frame
        for idx, ((x1, y1), _) in enumerate(fingers):
            cmds += [
                se(EV_ABS, SLOT, idx),
                se(EV_ABS, TID, idx),
                se(EV_ABS, MTX, rx(x1)),
                se(EV_ABS, MTY, ry(y1)),
                se(EV_ABS, PRESS, 64),
            ]
        cmds += [se(EV_KEY, BTN_TOUCH, 1), sync()]

        # Interpolated movement — all fingers in one sync frame per step
        for i in range(1, n_steps + 1):
            t = i / n_steps
            for idx, ((x1, y1), (x2, y2)) in enumerate(fingers):
                cmds += [
                    se(EV_ABS, SLOT, idx),
                    se(EV_ABS, MTX, rx(int(x1 + (x2 - x1) * t))),
                    se(EV_ABS, MTY, ry(int(y1 + (y2 - y1) * t))),
                ]
            cmds += [sync(), f"sleep {step_delay:.3f}"]

        # Touch up — all fingers in one sync frame
        for idx in range(len(fingers)):
            cmds += [se(EV_ABS, SLOT, idx), se(EV_ABS, TID, TID_NONE)]
        cmds += [se(EV_KEY, BTN_TOUCH, 0), sync()]

        await self._shell(" && ".join(cmds))

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
