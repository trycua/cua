#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Regression test: opening a window stream must not kill cua-spacesd.

WHY THIS SHAPE
--------------
The `sc_stream_create` ABI-mismatch bug (see
scripts/check-screencapturekit-unified.sh) killed `cua-spacesd` with SIGSEGV the
moment `open_session` reached `SCStream::new_with_delegate`. Three properties
made it invisible to the gates this project already had:

  1. No crash report was written in the guest on some images, so ".ips file
     exists" is not a usable detector.
  2. launchd restarts cua-spacesd within a second, so the daemon is listening again
     by the time anything polls a health endpoint.
  3. The client sees only `ConnectionClosedError: no close frame received or
     sent` -- indistinguishable from a network blip, and trivially swallowed
     by a test that just checks "did the call return".

So this test does not ask whether a call returned. It pins the daemon's PID
before the session and re-reads it after, and treats ANY change as failure --
that is the signal that survives all three properties above. It additionally
requires real frames to arrive, so a daemon that survives by never capturing
cannot pass either.

USAGE
  regress-open-session-survives.py <guest-ip> <token> [window-match]

Requires `sshpass` and the `websockets` package. Exits 0 on pass, 1 on fail.
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
WINDOW_MATCH = sys.argv[3] if len(sys.argv) > 3 else None

STREAM_SECONDS = 10.0
MIN_FRAMES = 10

# How to run a command in the guest. Defaults to password ssh, but hosts that
# have no `sshpass` (or that reach the guest only through lume's NAT) can set
# e.g. CUA_ENV_GUEST_SSH="lume ssh my-vm --" instead.
_ssh_override = os.environ.get("CUA_ENV_GUEST_SSH")
if _ssh_override:
    SSH = shlex.split(_ssh_override)
else:
    SSH = [
        "sshpass", "-p", "lume", "ssh",
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "IdentitiesOnly=yes",
        "-o", "ConnectTimeout=10",
        f"lume@{HOST}",
    ]


def daemon_pid():
    """PID of the running cua-spacesd in the guest, or None."""
    out = subprocess.run(
        SSH + ["pgrep -x cua-spacesd || true"],
        capture_output=True, text=True, timeout=30,
    )
    pids = [line.strip() for line in out.stdout.split() if line.strip().isdigit()]
    if not pids:
        return None
    # Lowest PID: if launchd has briefly double-started, we want a stable pick.
    return min(int(p) for p in pids)


def msg(t, payload=None):
    m = {"type": t}
    if payload is not None:
        m["payload"] = payload
    return json.dumps({"direction": "client", "message": m})


def parse_packet(data):
    hlen, plen = struct.unpack("!II", data[:8])
    return json.loads(data[8:8 + hlen]), data[8 + hlen:8 + hlen + plen]


async def stream_once():
    """Open a window session and count video frames. Returns (frames, note)."""
    url = f"ws://{HOST}:3211/ws"
    async with websockets.connect(
        url, max_size=64 * 1024 * 1024, ping_interval=None
    ) as ws:
        async def recv_control(kinds, timeout=20.0):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                raw = await asyncio.wait_for(
                    ws.recv(), timeout=deadline - time.monotonic()
                )
                if isinstance(raw, bytes):
                    continue
                m = json.loads(raw).get("message", {})
                if m.get("type") in kinds:
                    return m
                if m.get("type") == "error":
                    raise RuntimeError(f"server error: {m}")
            raise RuntimeError(f"timeout waiting for {kinds}")

        await ws.send(msg("authenticate", {"token": TOKEN}))
        await ws.send(msg("hello", {
            "protocol_name": "cua-media", "protocol_versions": [1], "capabilities": [],
        }))
        await recv_control({"welcome", "hello", "authenticated", "negotiated"})

        await ws.send(msg("list_windows", {"on_screen_only": True}))
        entries = (await recv_control({"windows"}))["payload"]["windows"]
        if WINDOW_MATCH:
            entries = [
                w for w in entries
                if WINDOW_MATCH.lower()
                in (str(w.get("application", "")) + str(w.get("title", ""))).lower()
            ]
        if not entries:
            raise RuntimeError("no matching window to stream")

        def area(w):
            g = w.get("geometry") or {}
            return (g.get("width") or 0) * (g.get("height") or 0)

        target = max(entries, key=area)
        print(f"  target: {target.get('application')} "
              f"{str(target.get('title'))[:50]} {target.get('geometry')}")

        await ws.send(msg("open_session", {
            "window": target["window"],
            "target_epoch": target["target_epoch"],
            "accepted_codecs": ["h264", "bgra", "png"],
            "max_fps": 30,
            "max_dimension": 1920,
            "policy": "allow_activation",
        }))
        opened = await recv_control({"session_opened"})
        geom = opened["payload"].get("geometry")
        print(f"  session opened, codec={opened['payload'].get('codec')} geom={geom}")

        frames = 0
        t0 = time.monotonic()
        while time.monotonic() - t0 < STREAM_SECONDS:
            try:
                raw = await asyncio.wait_for(ws.recv(), timeout=3.0)
            except asyncio.TimeoutError:
                continue
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
        return frames, geom


def main():
    print(f"== open_session survival check against {HOST}")

    pid_before = daemon_pid()
    if pid_before is None:
        print("FAIL: no cua-spacesd running in the guest before the test.")
        return 1
    print(f"  cua-spacesd pid before: {pid_before}")

    frames, geom, closed = 0, None, None
    try:
        frames, geom = asyncio.run(stream_once())
    except websockets.exceptions.ConnectionClosed as exc:
        closed = f"connection closed mid-session: {exc!r}"
        print(f"  {closed}")
    except Exception as exc:  # noqa: BLE001 - report anything, decide below
        closed = f"{type(exc).__name__}: {exc}"
        print(f"  error: {closed}")

    # Give launchd a beat to have done its restart, so a dead daemon shows up
    # as a CHANGED pid rather than as a missing one.
    time.sleep(3)
    pid_after = daemon_pid()
    print(f"  cua-spacesd pid after:  {pid_after}")
    print(f"  video frames received: {frames}")

    ok = True
    if pid_after is None:
        print("FAIL: cua-spacesd is not running after the session -- it died and did "
              "not come back.")
        ok = False
    elif pid_after != pid_before:
        print(f"FAIL: cua-spacesd pid changed {pid_before} -> {pid_after}. The daemon "
              "died during open_session and launchd restarted it. This is the "
              "sc_stream_create null-function-pointer segfault; note that no "
              "crash report is written on some images, so the pid change is "
              "the only reliable evidence.")
        ok = False
    else:
        print("PASS: cua-spacesd survived the session (pid unchanged).")

    if closed and ok:
        print(f"FAIL: session did not complete cleanly: {closed}")
        ok = False

    if frames < MIN_FRAMES:
        print(f"FAIL: only {frames} video frames in {STREAM_SECONDS:.0f}s "
              f"(need >= {MIN_FRAMES}). A daemon that survives by never "
              "capturing must not pass.")
        ok = False
    elif ok:
        print(f"PASS: {frames} frames delivered, geometry {geom}.")

    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
