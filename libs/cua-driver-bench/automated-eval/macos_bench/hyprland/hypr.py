# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""A small Hyprland IPC client (the request socket `hyprctl` uses), for the Hyprland port of BenchLab.

The session, the sentinel and the guard read the compositor through it: clients (in stacking order), the active
window and workspace, the cursor position, and dispatchers for window placement.
"""

from __future__ import annotations

import json
import os
import socket
from pathlib import Path
from typing import Any


def socket_path() -> Path:
    sig = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE", "")
    runtime = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    if not sig:
        found = sorted(
            Path(runtime, "hypr").glob("*/.socket.sock"), key=lambda p: p.stat().st_mtime
        )
        if not found:
            raise RuntimeError("no Hyprland instance (HYPRLAND_INSTANCE_SIGNATURE is unset)")
        return found[-1]
    return Path(runtime, "hypr", sig, ".socket.sock")


def request(command: str, timeout: float = 8.0) -> str:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.settimeout(timeout)
        conn.connect(str(socket_path()))
        conn.sendall(command.encode())
        chunks = []
        while True:
            chunk = conn.recv(65536)
            if not chunk:
                break
            chunks.append(chunk)
    return b"".join(chunks).decode(errors="replace")


def query(what: str) -> Any:
    return json.loads(request(f"j/{what}") or "null")


def clients() -> list[dict[str, Any]]:
    return [c for c in query("clients") or [] if c.get("mapped", True)]


def active_window() -> dict[str, Any]:
    return query("activewindow") or {}


def active_workspace() -> dict[str, Any]:
    return query("activeworkspace") or {}


def cursor() -> tuple[float, float]:
    pos = query("cursorpos") or {}
    return float(pos.get("x", 0)), float(pos.get("y", 0))


def dispatch(expression: str) -> str:
    """Run one dispatcher. Hyprland 0.55+ with a Lua config takes Lua (`hl.dsp.window.close()`)."""
    return request("dispatch " + expression)


def window_ref(address: str) -> str:
    return f'window = "address:{address}"'


def by_pid(pid: int) -> list[dict[str, Any]]:
    return [c for c in clients() if c.get("pid") == pid]


def place(address: str, frame: tuple[int, int, int, int]) -> None:
    """Float the window and give it an exact frame (x, y, w, h) in layout coordinates."""
    x, y, w, h = frame
    ref = window_ref(address)
    dispatch(f'hl.dsp.window.float({{ action = "on", {ref} }})')
    dispatch(f"hl.dsp.window.resize({{ x = {w}, y = {h}, {ref} }})")
    dispatch(f"hl.dsp.window.move({{ x = {x}, y = {y}, {ref} }})")


def focus(address: str) -> None:
    dispatch(f"hl.dsp.focus({{ {window_ref(address)} }})")


def raise_to_top(address: str) -> None:
    dispatch(f'hl.dsp.window.alter_zorder({{ mode = "top", {window_ref(address)} }})')


def overlaps(a: dict[str, Any], b: dict[str, Any]) -> bool:
    (ax, ay), (aw, ah) = a.get("at", (0, 0)), a.get("size", (0, 0))
    (bx, by), (bw, bh) = b.get("at", (0, 0)), b.get("size", (0, 0))
    return ax < bx + bw and bx < ax + aw and ay < by + bh and by < ay + ah


def identity(client: dict[str, Any]) -> str:
    """The name a window is reported under: its class (the app id), else its title, else its address."""
    return (
        client.get("class")
        or client.get("initialClass")
        or client.get("title")
        or client.get("address", "")
    )
