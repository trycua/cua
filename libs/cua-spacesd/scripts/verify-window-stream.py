#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Release verification for the macOS window-capture path.

Checks, against a live guest, the things that the `sc_stream_create` segfault
made impossible:

  1. `open_session` succeeds and cua-spacesd SURVIVES it (pid pinned before/after --
     see regress-open-session-survives.py for why pid is the right detector).
  2. Video frames are actually delivered. ScreenCaptureKit only emits on
     content change, so this script MAKES the window change: it drives the
     real pointer across the target through `interactive_input`, which both
     dirties pixels and moves the guest's physical cursor.
  3. The cursor SHAPE reaches a viewer. A second connection listens for
     `remote_cursor` broadcasts, which carry `CursorState.shape`. Hovering a
     text area must report `text` (NSCursor.iBeam). This is the part that was
     unprovable before the fix, because the shape is only sampled while a
     capture session holds the window geometry.

Usage:
  verify-window-stream.py <guest-ip> <token> [window-title-substring]

Exit 0 only if every check passes.
"""
import asyncio
import json
import os
import shlex
import struct
import subprocess
import sys
import time

import websockets

HOST = sys.argv[1]
TOKEN = sys.argv[2]
MATCH = sys.argv[3] if len(sys.argv) > 3 else None
URL = f"ws://{HOST}:3211/ws"

DRIVE_SECONDS = 12.0
MIN_FRAMES = 10

_ssh = os.environ.get("CUA_ENV_GUEST_SSH")
SSH = shlex.split(_ssh) if _ssh else [
    "sshpass", "-p", "lume", "ssh", "-o", "StrictHostKeyChecking=no",
    "-o", "UserKnownHostsFile=/dev/null", "-o", "IdentitiesOnly=yes",
    f"lume@{HOST}",
]


def daemon_pid():
    out = subprocess.run(SSH + ["pgrep -x cua-spacesd || true"],
                         capture_output=True, text=True, timeout=30)
    pids = [p for p in out.stdout.split() if p.strip().isdigit()]
    return min(int(p) for p in pids) if pids else None


def envelope(t, payload=None):
    m = {"type": t}
    if payload is not None:
        m["payload"] = payload
    return json.dumps({"direction": "client", "message": m})


def parse_packet(data):
    hlen, plen = struct.unpack("!II", data[:8])
    return json.loads(data[8:8 + hlen]), data[8 + hlen:8 + hlen + plen]


async def handshake(ws, name):
    await ws.send(envelope("authenticate", {"token": TOKEN}))
    await ws.send(envelope("hello", {
        "protocol_name": "cua-media", "protocol_versions": [1], "capabilities": [],
    }))
    await ws.send(envelope("join", {"name": name, "color": "#44aa88"}))


async def recv_until(ws, kinds, timeout=20.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        raw = await asyncio.wait_for(ws.recv(), timeout=deadline - time.monotonic())
        if isinstance(raw, bytes):
            continue
        m = json.loads(raw).get("message", {})
        if m.get("type") in kinds:
            return m
        if m.get("type") == "error":
            raise RuntimeError(f"server error: {m}")
    raise RuntimeError(f"timeout waiting for {kinds}")


async def viewer(shapes, stop):
    """Second connection: record every cursor shape broadcast to a viewer."""
    async with websockets.connect(URL, max_size=8 * 1024 * 1024,
                                  ping_interval=None) as ws:
        await handshake(ws, "viewer")
        while not stop.is_set():
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=1.0)
            except asyncio.TimeoutError:
                continue
            if isinstance(raw, bytes):
                continue
            m = json.loads(raw).get("message", {})
            if m.get("type") == "remote_cursor":
                p = m.get("payload", m)
                shape = (p.get("shape") or {}).get("kind")
                if shape:
                    # Only the "host" pseudo-user owns the origin desktop's one
                    # physical pointer, so only its broadcasts carry a real
                    # shape; a participant's own cursor is always Unknown by
                    # design (presence.rs). Keep the user_id so the check below
                    # cannot be satisfied by a participant echo.
                    shapes.append((p.get("user_id"), shape,
                                   p.get("window") is not None))
            elif m.get("type") == "error":
                print(f"  [viewer] server error: {m}")


async def driver(result):
    async with websockets.connect(URL, max_size=64 * 1024 * 1024,
                                  ping_interval=None) as ws:
        await handshake(ws, "driver")
        await ws.send(envelope("list_windows", {"on_screen_only": True}))
        wins = (await recv_until(ws, {"windows"}))["payload"]["windows"]
        if MATCH:
            wins = [w for w in wins if MATCH.lower() in
                    (str(w.get("application", "")) + str(w.get("title", ""))).lower()]
        if not wins:
            raise RuntimeError("no matching window")

        def area(w):
            g = w.get("geometry") or {}
            return (g.get("width_px") or 0) * (g.get("height_px") or 0)

        target = max(wins, key=area)
        result["target"] = f"{target.get('application')} / {target.get('title')}"
        print(f"  target: {result['target']} {target.get('geometry')}")

        await ws.send(envelope("open_session", {
            "window": target["window"],
            "target_epoch": target["target_epoch"],
            "accepted_codecs": ["h264", "bgra", "png"],
            "max_fps": 30,
            "max_dimension": 1920,
            "policy": "allow_activation",
        }))
        opened = await recv_until(ws, {"session_opened"})
        p = opened["payload"]
        sid = p.get("session_id")
        result["codec"] = p.get("codec")
        result["geometry"] = p.get("geometry")
        print(f"  session opened: codec={p.get('codec')} geom={p.get('geometry')}")

        seq = 0
        frames = 0
        t0 = time.monotonic()
        # Sweep the pointer horizontally across the vertical middle of the
        # window, which for a TextEdit document is its text area.
        while time.monotonic() - t0 < DRIVE_SECONDS:
            phase = (time.monotonic() - t0) / DRIVE_SECONDS
            xn = 0.2 + 0.6 * abs(((phase * 4) % 2) - 1)
            yn = 0.55
            await ws.send(envelope("interactive_input", {
                "session_id": sid,
                "first_sequence": seq,
                "events": [{
                    "kind": "pointer", "phase": "move", "button": None,
                    "x_normalized": xn, "y_normalized": yn, "modifiers": [],
                }],
            }))
            seq += 1
            await ws.send(envelope("cursor", {
                "window": target["window"],
                "x": xn * (result["geometry"] or {}).get("width_px", 600),
                "y": yn * (result["geometry"] or {}).get("height_px", 500),
                "visible": True, "pressed": False,
            }))
            # The macOS cursor SHAPE only reaches viewers through cua-driver's
            # cursor hook (cua-spacesd-desktop::on_cursor_event), and that hook
            # fires for driver-issued actions -- not for the low-level
            # `interactive_input` path, which posts events directly. So also
            # drive a real click through the action provider, which moves the
            # physical pointer into the text area and makes the OS swap in the
            # I-beam. Needs the capture geometry from the open session, which
            # is exactly what the segfault used to prevent.
            g = result.get("geometry") or {}
            await ws.send(envelope("action", {
                "action_id": f"probe-{seq}",
                "session_id": sid,
                "tool": "click",
                "arguments": {
                    "x": xn * g.get("width_px", 600),
                    "y": yn * g.get("height_px", 500),
                },
                "basis": {"kind": "none"},
            }))
            deadline = time.monotonic() + 0.15
            while time.monotonic() < deadline:
                try:
                    raw = await asyncio.wait_for(
                        ws.recv(), timeout=max(0.01, deadline - time.monotonic()))
                except asyncio.TimeoutError:
                    break
                if not isinstance(raw, bytes):
                    continue
                header, _ = parse_packet(raw)
                d = header.get("video_frame") or header
                if isinstance(d, dict) and "capture_timestamp_us" not in d:
                    for v in d.values():
                        if isinstance(v, dict) and "capture_timestamp_us" in v:
                            d = v
                            break
                if isinstance(d, dict) and "capture_timestamp_us" in d:
                    frames += 1
        result["frames"] = frames


async def run(result, shapes):
    stop = asyncio.Event()
    vt = asyncio.create_task(viewer(shapes, stop))
    await asyncio.sleep(1.0)  # let the viewer join before the driver moves
    try:
        await driver(result)
    finally:
        stop.set()
        await asyncio.wait_for(vt, timeout=10)


def main():
    print(f"== window stream verification against {HOST}")
    pid_before = daemon_pid()
    print(f"  cua-spacesd pid before: {pid_before}")
    if pid_before is None:
        print("FAIL: cua-spacesd not running")
        return 1

    result, shapes = {"frames": 0}, []
    err = None
    try:
        asyncio.run(run(result, shapes))
    except Exception as exc:  # noqa: BLE001
        err = f"{type(exc).__name__}: {exc}"
        print(f"  error: {err}")

    time.sleep(3)
    pid_after = daemon_pid()
    print(f"  cua-spacesd pid after:  {pid_after}")

    ok = True
    if pid_after != pid_before:
        print(f"FAIL: cua-spacesd pid changed {pid_before} -> {pid_after} (daemon died)")
        ok = False
    else:
        print("PASS: cua-spacesd survived the session")

    if err:
        print(f"FAIL: session error: {err}")
        ok = False

    frames = result.get("frames", 0)
    if frames >= MIN_FRAMES:
        print(f"PASS: {frames} video frames delivered "
              f"(codec {result.get('codec')}, geom {result.get('geometry')})")
    else:
        print(f"FAIL: only {frames} video frames (need >= {MIN_FRAMES})")
        ok = False

    # Provenance, not identity, is what makes this check meaningful. cua-spacesd has
    # exactly two producers of RemoteCursor: the presence echo of a client's
    # own `cursor` message, which hard-codes CursorShape::Unknown
    # (cua-spacesd/src/presence.rs), and the cua-driver cursor hook, which reads the
    # real system shape (cua-spacesd-desktop::on_cursor_event). So ANY shape
    # other than "unknown" can only have come from the hook -- a client cannot
    # forge one by echoing its own cursor.
    real = [s for s in shapes if s[1] != "unknown"]
    kinds = sorted({s[1] for s in real})
    carriers = sorted({s[0] for s in real if s[1] == "text"})
    print(f"  real cursor shapes seen by the viewer: {kinds} "
          f"({len(real)} hook-sourced updates of {len(shapes)} total)")
    if carriers:
        print(f"  'text' carried by cursor identities: {carriers}")
    if "text" in kinds:
        print("PASS: cursor shape 'text' reached the viewer over the text area")
    else:
        print("FAIL: viewer never saw cursor shape 'text' over the text area")
        ok = False

    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
