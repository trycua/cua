# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Scripted oracle for the BenchLab modes on Hyprland: solves a task through the GUI with real input.

Used only to validate the port (each task solvable, its checker passes); never a bench setup. It finds controls
through AT-SPI (window-relative extents plus the window's frame from Hyprland) and acts in the foreground with a
uinput device: an absolute pointer with buttons and a keyboard, the same kernel path as a USB mouse and keyboard.
The desktop user needs read-write access to /dev/uinput while it runs.

Usage: oracle.py --mode MODE --seed N [--disturb KIND]
`--disturb KIND` instead performs one deliberate disturbance of the user window (validates the sentinel):
focus, raise, pointer, clipboard, typing, space or all.
"""

from __future__ import annotations

import argparse
import ctypes
import fcntl
import os
import struct
import subprocess
import sys
import time
from pathlib import Path

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "probes" / "_common"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import benchlab_common as C  # noqa: E402
import hypr  # noqa: E402

EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
ABS_X, ABS_Y = 0, 1
BTN_LEFT = 0x110
UI_SET_EVBIT, UI_SET_KEYBIT, UI_SET_ABSBIT = 0x40045564, 0x40045565, 0x40045567
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
KEYS = {
    "ctrl": 29,
    "a": 30,
    "backspace": 14,
    "down": 108,
    "up": 103,
    "enter": 28,
    "tab": 15,
    "space": 57,
}


class Input:
    """A uinput tablet-style pointer (absolute, mapped to the whole layout) and keyboard."""

    def __init__(self) -> None:
        width, height = self.layout()
        self.size = (width, height)
        self.fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
        fcntl.ioctl(self.fd, UI_SET_EVBIT, EV_KEY)
        fcntl.ioctl(self.fd, UI_SET_EVBIT, EV_ABS)
        for code in (BTN_LEFT, BTN_LEFT + 1, *KEYS.values()):
            fcntl.ioctl(self.fd, UI_SET_KEYBIT, code)
        fcntl.ioctl(self.fd, UI_SET_ABSBIT, ABS_X)
        fcntl.ioctl(self.fd, UI_SET_ABSBIT, ABS_Y)
        absmax = [0] * 64
        absmax[ABS_X], absmax[ABS_Y] = width - 1, height - 1
        dev = struct.pack("80sHHHHi", b"cdb-oracle", 0x03, 0x1234, 0x5678, 1, 0)
        dev += struct.pack("64i", *absmax) + struct.pack("64i", *([0] * 64)) * 3
        os.write(self.fd, dev)
        fcntl.ioctl(self.fd, UI_DEV_CREATE)
        time.sleep(1.0)  # the compositor picks the device up

    @staticmethod
    def layout() -> tuple[int, int]:
        monitors = hypr.query("monitors") or []
        return (
            max(m["x"] + m["width"] for m in monitors),
            max(m["y"] + m["height"] for m in monitors),
        )

    def emit(self, kind: int, code: int, value: int) -> None:
        os.write(self.fd, struct.pack("llHHi", 0, 0, kind, code, value))

    def sync(self) -> None:
        self.emit(EV_SYN, 0, 0)

    def move(self, x: float, y: float) -> None:
        self.emit(EV_ABS, ABS_X, int(x))
        self.emit(EV_ABS, ABS_Y, int(y))
        self.sync()
        time.sleep(0.03)

    def button(self, down: bool) -> None:
        self.emit(EV_KEY, BTN_LEFT, 1 if down else 0)
        self.sync()
        time.sleep(0.05)

    def click(self, x: float, y: float) -> None:
        self.move(x, y)
        time.sleep(0.15)
        self.button(True)
        self.button(False)
        time.sleep(0.3)

    def key(self, name: str, *mods: str) -> None:
        for m in mods:
            self.emit(EV_KEY, KEYS[m], 1)
        self.emit(EV_KEY, KEYS[name], 1)
        self.sync()
        self.emit(EV_KEY, KEYS[name], 0)
        for m in mods:
            self.emit(EV_KEY, KEYS[m], 0)
        self.sync()
        time.sleep(0.05)

    def close(self) -> None:
        fcntl.ioctl(self.fd, UI_DEV_DESTROY)
        os.close(self.fd)


IN: Input | None = None


def type_text(text: str) -> None:
    # wtype: Wayland virtual-keyboard input to the focused surface (any character, no keymap table here).
    subprocess.run(["wtype", "-d", "20", text], check=True, timeout=60)
    time.sleep(0.2)


def app() -> Atspi.Accessible:
    desktop = Atspi.get_desktop(0)
    for _ in range(50):
        for i in range(desktop.get_child_count()):
            child = desktop.get_child_at_index(i)
            if child is not None and child.get_name() in ("BenchLab", "benchlab.py"):
                return child
        time.sleep(0.2)
    raise RuntimeError("BenchLab is not on the accessibility bus")


def walk(node: Atspi.Accessible):
    yield node
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from walk(child)


def toplevel(title_prefix: str) -> tuple[Atspi.Accessible, dict]:
    for _ in range(80):
        root = app()
        for i in range(root.get_child_count()):
            win = root.get_child_at_index(i)
            if win is not None and win.get_name().startswith(title_prefix):
                frames = [c for c in hypr.clients() if c.get("title", "").startswith(title_prefix)]
                if frames:
                    return win, frames[-1]
        time.sleep(0.25)
    raise RuntimeError(f"no window {title_prefix!r}")


ROLE_ALIASES = {"push button": ("push button", "button"), "check box": ("check box", "checkbox")}


def find(win: Atspi.Accessible, role: str, name: str, nth: int = 0) -> Atspi.Accessible:
    roles = ROLE_ALIASES.get(role, (role,))
    hits = [
        n for n in walk(win)
        if n.get_role_name() in roles and n.get_name() == name and n.get_state_set().contains(Atspi.StateType.SHOWING)
    ]  # fmt: skip
    if len(hits) <= nth:
        raise RuntimeError(f"no {role} {name!r}")
    return hits[nth]


def center(node: Atspi.Accessible, frame: dict) -> tuple[float, float]:
    box = node.get_extents(Atspi.CoordType.WINDOW)
    x0, y0 = frame["at"]
    return x0 + box.x + box.width / 2, y0 + box.y + box.height / 2


def lab(mode: str) -> tuple[Atspi.Accessible, dict]:
    win, frame = toplevel(f"BenchLab — {mode}")
    hypr.focus(frame["address"])
    hypr.raise_to_top(frame["address"])
    time.sleep(0.4)
    return win, frame


def click(node: Atspi.Accessible, frame: dict) -> None:
    IN.click(*center(node, frame))


def button(mode: str, title: str) -> None:
    win, frame = lab(mode)
    click(find(win, "push button", title), frame)


def type_into(mode: str, name: str, value: str) -> None:
    win, frame = lab(mode)
    click(find(win, "text", name), frame)
    IN.key("a", "ctrl")
    IN.key("backspace")
    type_text(str(value))


def choose(
    mode: str, name: str, choices: list[str], value: str, current: str | None = None
) -> None:
    """Open the combo box (its popup opens on the current item) and step to VALUE with the arrow keys."""
    win, frame = lab(mode)
    click(find(win, "combo box", name), frame)
    time.sleep(0.5)
    steps = choices.index(value) - choices.index(current or choices[0])
    for _ in range(abs(steps)):
        IN.key("down" if steps > 0 else "up")
        time.sleep(0.1)
    IN.key("enter")
    time.sleep(0.4)


def dialog_button(title_prefix: str, label: str) -> None:
    win, frame = toplevel(title_prefix)
    hypr.focus(frame["address"])
    time.sleep(0.3)
    click(find(win, "push button", label), frame)


def drag(src: tuple[float, float], dst: tuple[float, float]) -> None:
    IN.move(*src)
    time.sleep(0.2)
    IN.button(True)
    for i in range(1, 21):
        IN.move(src[0] + (dst[0] - src[0]) * i / 20, src[1] + (dst[1] - src[1]) * i / 20)
        time.sleep(0.03)
    time.sleep(0.3)
    IN.button(False)
    time.sleep(0.6)


def listdrag(seed: int) -> None:
    p = C.derive_listdrag(seed)
    order = list(p["initial"])
    for index, item in enumerate(p["target"]):
        source = order.index(item)
        if source == index:
            continue
        win, frame = lab("listdrag")
        rows = [find(win, "label", name) for name in order]
        drag(center(rows[source], frame), center(rows[index], frame))
        order.pop(source)
        order.insert(index, item)
    button("listdrag", "Done")


def hover(seed: int) -> None:
    p = C.derive_hover(seed)
    win, frame = lab("hover")
    x, y = center(find(win, "label", "Hover here to reveal actions"), frame)
    IN.move(x, y)
    time.sleep(0.6)
    IN.move(x + 2, y + 20)
    time.sleep(0.6)
    win, frame = lab("hover")
    click(find(win, "push button", p["target"]), frame)


def tooltip(seed: int) -> None:
    p = C.derive_tooltip(seed)
    win, frame = lab("tooltip")
    x, y = center(find(win, "label", p["target"]), frame)
    IN.move(x - 3, y)
    time.sleep(0.2)
    IN.move(x, y)
    time.sleep(2.0)
    type_into("tooltip", "Code", p["target_code"])
    button("tooltip", "Submit")


def irmodal(seed: int) -> None:
    p = C.derive_irmodal(seed)
    type_into("irmodal", "Name", p["name"])
    type_into("irmodal", "Email", p["email"])
    button("irmodal", "Next")
    dialog_button("Allow BenchLab", "Don’t Allow")
    choose("irmodal", "Team", C.IR_TEAMS, p["team"])
    type_into("irmodal", "Seats", str(p["seats"]))
    button("irmodal", "Submit")


def irbanner(seed: int) -> None:
    p = C.derive_irbanner(seed)
    type_into("irbanner", "Quantity", str(p["quantity"]))
    time.sleep(0.6)
    button("irbanner", "Dismiss")
    choose("irbanner", "Priority", C.PRIORITIES, p["priority"])
    button("irbanner", "Apply")


def irunsaved(seed: int) -> None:
    p = C.derive_irunsaved(seed)
    statuses = [C.IR_INITIAL_STATUS, *C.IR_STATUS_WORDS]
    button("irunsaved", p["titles"][p["status_index"]])
    choose("irunsaved", "Status", statuses, p["status"])
    button("irunsaved", p["titles"][p["rename_index"]])
    dialog_button("Save changes", "Save")
    type_into("irunsaved", "Title", p["new_title"])
    button("irunsaved", "Done")
    dialog_button("Save changes", "Save")


def irconsent(seed: int) -> None:
    p = C.derive_irconsent(seed)
    type_into("irconsent", "Email", p["email"])
    dialog_button("Cookie preferences", "Reject optional")
    type_into(
        "irconsent", "Email", p["email"]
    )  # keys typed while the modal prompt was open went nowhere
    choose("irconsent", "Plan", C.IR_PLANS, p["plan"])
    win, frame = lab("irconsent")
    box = find(win, "check box", "Email digest")
    if box.get_state_set().contains(Atspi.StateType.CHECKED) != p["digest"]:
        click(box, frame)
    button("irconsent", "Sign up")


def disturb(kind: str) -> None:
    """One deliberate disturbance of the user window, of the given kind."""
    time.sleep(2)
    lab_frame = next(c for c in hypr.clients() if c.get("class") == "BenchLab")
    if kind in ("focus", "all"):
        hypr.focus(lab_frame["address"])  # key focus and activation move to BenchLab
    if kind in ("raise", "all"):
        hypr.raise_to_top(lab_frame["address"])  # BenchLab drawn over the user window
    if kind in ("pointer", "all"):
        IN.move(300, 300)
        IN.move(320, 310)
    if kind in ("clipboard", "all"):
        subprocess.run(["wl-copy", "cdb-disturbance"], check=True, timeout=10)
    if kind == "typing":
        type_text("x")  # a keystroke lands in the user window
    if kind == "space":
        hypr.dispatch('hl.dsp.focus({ workspace = "2" })')
        time.sleep(1.5)
        hypr.dispatch('hl.dsp.focus({ workspace = "1" })')
    time.sleep(2)


def main() -> None:
    global IN
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", required=True)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--disturb")
    args = parser.parse_args()
    ctypes.CDLL(None).prctl(15, b"cdb-oracle", 0, 0, 0)
    Atspi.init()
    IN = Input()
    try:
        if args.disturb:
            disturb(args.disturb)
        else:
            globals()[args.mode](args.seed)
        time.sleep(1.0)
    finally:
        IN.close()


if __name__ == "__main__":
    main()
