"""The neutral ``computer/v1`` operations on macOS, for drivers whose software brings its own tools.

agent-desktop, the browser-use macOS harness and Codex Computer Use each serve their own tools to an agent. AgentEnv
still needs the seven ``computer_*`` operations from every driver: for recordings, the conformance probe and the
readiness checks. This base class serves them from two things every Mac has, and neither belongs to any arm:

* ``/usr/sbin/screencapture`` takes the screenshot;
* Quartz events (CGEventPost) send the pointer and keyboard input.

An env hides these tools from its agent with its ``[tools]`` list, so each arm is scored on its own tools only.
Run with the driver's own Screen Recording and Accessibility grants, in the VM's GUI session.

This file is the same in each driver's folder, since a driver installs from its own folder; a test checks that.
"""

from __future__ import annotations

import asyncio
import os
import subprocess
import tempfile
from pathlib import Path
from typing import Any, Protocol

from agentenv_protocol.computer import (
    PRINTABLE_KEYS,
    ComputerDriverEnvironment,
    DeliveryFailed,
    DeviceUnavailable,
    Display,
    Image,
    Unsupported,
    capture,
    click,
    display,
    drag,
    press,
    run_in_session,
    scroll,
    type_text,
    write_files,
)

#: How much of a command's output computer-exec keeps.
EXEC_OUTPUT_BYTES = 1 << 20
SCREENCAPTURE = "/usr/sbin/screencapture"
CAPTURE_TIMEOUT_S = 20

#: Quartz's CGEventFlags masks.
FLAG_SHIFT, FLAG_CONTROL, FLAG_OPTION, FLAG_COMMAND = 0x20000, 0x40000, 0x80000, 0x100000
#: W3C modifier key values and the flag each holds down (Meta and Super are Command).
MODIFIERS = {
    "Shift": FLAG_SHIFT,
    "Control": FLAG_CONTROL,
    "Alt": FLAG_OPTION,
    "Meta": FLAG_COMMAND,
    "Super": FLAG_COMMAND,
}
#: Virtual key codes on a US keyboard (kVK_ANSI_*), by the character the key types unshifted.
_ANSI = {
    **dict(zip("asdfhgzxcvb", (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11))),
    **dict(zip("qwertyuiop", (12, 13, 14, 15, 17, 16, 32, 34, 31, 35))),
    **dict(zip("123456", (18, 19, 20, 21, 23, 22))),
    **dict(zip("lkjnm", (37, 40, 38, 45, 46))),
    "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "[": 33, "'": 39, ";": 41,
    "\\": 42, ",": 43, "/": 44, ".": 47, "`": 50,
}  # fmt: skip
#: A shifted character, and the key that types it unshifted.
SHIFTED = {
    **{c.upper(): c for c in "abcdefghijklmnopqrstuvwxyz"},
    **dict(zip('!@#$%^&*()_+{}|:"<>?~', r"1234567890-=[]\;',./`")),
}
#: Named W3C key values and their virtual key codes.
NAMED = {
    "Enter": 36, "Tab": 48, " ": 49, "Backspace": 51, "Escape": 53, "Delete": 117, "Home": 115,
    "End": 119, "PageUp": 116, "PageDown": 121, "ArrowLeft": 123, "ArrowRight": 124,
    "ArrowDown": 125, "ArrowUp": 126,
    "F1": 122, "F2": 120, "F3": 99, "F4": 118, "F5": 96, "F6": 97, "F7": 98, "F8": 100, "F9": 101,
    "F10": 109, "F11": 103, "F12": 111,
}  # fmt: skip
BUTTONS = {"left": 0, "right": 1, "middle": 2}


def resolve_key(key: str) -> tuple[int, int]:
    """The virtual key code and extra modifier flags that press one W3C key value."""
    if key in NAMED:
        return NAMED[key], 0
    if key in SHIFTED:
        return _ANSI[SHIFTED[key]], FLAG_SHIFT
    if key.lower() in _ANSI and len(key) == 1:
        return _ANSI[key.lower()], 0
    raise Unsupported(f"no key code for {key!r} on this keyboard")


def resolve_chord(keys: list[str]) -> tuple[int, int]:
    """The key code and modifier flags of a chord: its modifiers, then the key pressed with them."""
    *held, key = keys
    flags = 0
    for modifier in held:
        if modifier not in MODIFIERS:
            raise Unsupported(f"{modifier!r} can be held, but it isn't a modifier key")
        flags |= MODIFIERS[modifier]
    if key in MODIFIERS:
        raise Unsupported("a chord ends in a key that isn't a modifier")
    code, extra = resolve_key(key)
    return code, flags | extra


def modifier_flags(keys: list[str]) -> int:
    flags = 0
    for modifier in keys:
        if modifier not in MODIFIERS:
            raise Unsupported(f"{modifier!r} can be held, but it isn't a modifier key")
        flags |= MODIFIERS[modifier]
    return flags


class Backend(Protocol):
    """What the operations need of the Mac; ``QuartzBackend`` is the real one and a test gives a fake."""

    def screen(self) -> tuple[int, int, float]: ...
    def grab(self) -> tuple[int, int, bytes]: ...
    def click(self, x: float, y: float, button: int, count: int, flags: int) -> None: ...
    def drag(self, path: list, button: int, duration_ms: int, flags: int) -> None: ...
    def scroll(self, x: float, y: float, dx: int, dy: int, flags: int) -> None: ...
    def type_text(self, text: str) -> None: ...
    def key(self, code: int, flags: int) -> None: ...


class QuartzBackend:
    """``screencapture`` for the screen and Quartz events for input. Needs pyobjc-framework-Quartz."""

    def __init__(self) -> None:
        try:
            import Quartz
        except ImportError as e:
            raise DeviceUnavailable(f"Quartz isn't importable: {e}") from None
        self.q = Quartz

    def screen(self) -> tuple[int, int, float]:
        q, main = self.q, self.q.CGMainDisplayID()
        bounds = q.CGDisplayBounds(main)
        width, height = int(bounds.size.width), int(bounds.size.height)
        pixels = q.CGDisplayPixelsWide(main)
        return width, height, (pixels / width) if width and pixels else 1.0

    def grab(self) -> tuple[int, int, bytes]:
        with tempfile.TemporaryDirectory(prefix="agentenv-cap-") as folder:
            path = Path(folder) / "screen.png"
            done = subprocess.run(
                [SCREENCAPTURE, "-x", "-t", "png", str(path)],
                capture_output=True,
                timeout=CAPTURE_TIMEOUT_S,
            )
            if done.returncode or not path.exists():
                raise DeviceUnavailable(
                    f"screencapture failed ({done.returncode}): {done.stderr.decode(errors='replace')[:200]}"
                )
            data = path.read_bytes()
        width, height = int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")
        return width, height, data

    def _post(self, event: Any, flags: int) -> None:
        self.q.CGEventSetFlags(event, flags)
        self.q.CGEventPost(self.q.kCGHIDEventTap, event)

    def _mouse(
        self, kind: int, x: float, y: float, button: int, flags: int, count: int = 1
    ) -> None:
        event = self.q.CGEventCreateMouseEvent(None, kind, (x, y), button)
        self.q.CGEventSetIntegerValueField(event, self.q.kCGMouseEventClickState, count)
        self._post(event, flags)

    def _kinds(self, button: int) -> tuple[int, int, int]:
        q = self.q
        return [
            (q.kCGEventLeftMouseDown, q.kCGEventLeftMouseUp, q.kCGEventLeftMouseDragged),
            (q.kCGEventRightMouseDown, q.kCGEventRightMouseUp, q.kCGEventRightMouseDragged),
            (q.kCGEventOtherMouseDown, q.kCGEventOtherMouseUp, q.kCGEventOtherMouseDragged),
        ][button]

    def click(self, x: float, y: float, button: int, count: int, flags: int) -> None:
        down, up, _ = self._kinds(button)
        self._mouse(self.q.kCGEventMouseMoved, x, y, button, flags)
        for n in range(1, count + 1):
            self._mouse(down, x, y, button, flags, n)
            self._mouse(up, x, y, button, flags, n)

    def drag(self, path: list, button: int, duration_ms: int, flags: int) -> None:
        import time

        down, up, dragged = self._kinds(button)
        (x0, y0), (x1, y1) = path[0], path[-1]
        self._mouse(self.q.kCGEventMouseMoved, x0, y0, button, flags)
        self._mouse(down, x0, y0, button, flags)
        steps = max(2, min(60, duration_ms // 16))
        for i in range(1, steps + 1):
            t = i / steps
            self._mouse(dragged, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, button, flags)
            time.sleep(duration_ms / 1000 / steps)
        self._mouse(up, x1, y1, button, flags)

    def scroll(self, x: float, y: float, dx: int, dy: int, flags: int) -> None:
        self._mouse(self.q.kCGEventMouseMoved, x, y, 0, flags)
        event = self.q.CGEventCreateScrollWheelEvent(None, self.q.kCGScrollEventUnitLine, 2, dy, dx)
        self._post(event, flags)

    def type_text(self, text: str) -> None:
        for char in text:
            for is_down in (True, False):
                event = self.q.CGEventCreateKeyboardEvent(None, 0, is_down)
                self.q.CGEventKeyboardSetUnicodeString(event, len(char), char)
                self._post(event, 0)

    def key(self, code: int, flags: int) -> None:
        for is_down in (True, False):
            self._post(self.q.CGEventCreateKeyboardEvent(None, code, is_down), flags)


class MacComputerDriver(ComputerDriverEnvironment):
    """The seven ``computer_*`` operations on macOS, plus computer-exec and the data plane's file writes. A driver
    subclasses it, declares itself with ``@computer_driver``, and serves its own tools beside these."""

    buttons = ("left", "right", "middle")
    max_click_count = 3
    max_scroll_amount = 50
    keys = (*MODIFIERS, *NAMED, *PRINTABLE_KEYS)
    #: Makes the backend on first use; a test sets it to a fake.
    backend_factory: Any = QuartzBackend

    def __init__(self) -> None:
        super().__init__()
        self._backend: Backend | None = None

    def mac(self) -> Backend:
        """The backend, made on first use (a card is declared without ``__init__``, so this isn't a property)."""
        if getattr(self, "_backend", None) is None:
            self._backend = self.backend_factory()
        return self._backend

    @display
    async def screen(self) -> Display:
        width, height, scale = await asyncio.to_thread(self.mac().screen)
        return Display(width, height, scale)

    @capture
    async def grab(self) -> Image:
        width, height, data = await asyncio.to_thread(self.mac().grab)
        return Image(width, height, data, "png")

    @click
    async def click_at(self, x, y, button, count, modifiers) -> None:
        await self._input(self.mac().click, x, y, BUTTONS[button], count, modifier_flags(modifiers))

    @drag
    async def drag_along(self, path, button, duration_ms, modifiers) -> None:
        await self._input(
            self.mac().drag, list(path), BUTTONS[button], duration_ms, modifier_flags(modifiers)
        )

    @scroll
    async def scroll_at(self, x, y, direction, amount, modifiers) -> None:
        dx, dy = {
            "up": (0, amount),
            "down": (0, -amount),
            "left": (amount, 0),
            "right": (-amount, 0),
        }[direction]
        await self._input(self.mac().scroll, x, y, dx, dy, modifier_flags(modifiers))

    @type_text
    async def type_in(self, text) -> None:
        await self._input(self.mac().type_text, text)

    @press
    async def press_keys(self, keys) -> None:
        code, flags = resolve_chord(list(keys))
        await self._input(self.mac().key, code, flags)

    @run_in_session(os="macos")
    async def run_command(self, argv, cwd, env, timeout_s):
        """computer-exec: the driver runs in the GUI session as its user, so a command it starts does too."""
        from agentenv_protocol.computer import _linux  # POSIX: nothing in it is Linux-specific

        return await _linux.run_in_session(
            argv,
            cwd,
            env,
            timeout_s,
            output_bytes=getattr(self.settings, "exec_output_bytes", EXEC_OUTPUT_BYTES),
        )

    @write_files(os="macos")
    async def write_home_files(self, files) -> None:
        from agentenv_protocol.computer import _linux

        await asyncio.to_thread(_linux.write_files, files)

    @staticmethod
    async def _input(fn, *args) -> None:
        try:
            await asyncio.to_thread(fn, *args)
        except (DeviceUnavailable, Unsupported):
            raise
        except Exception as e:  # Quartz raises nothing useful; a failed post is a failed delivery
            raise DeliveryFailed(f"{type(e).__name__}: {e}") from None


def home_path(*parts: str) -> Path:
    return Path(os.path.expanduser("~")).joinpath(*parts)
