#!/usr/bin/env python3
"""Minimal RFB (VNC) client: connect, grab one full frame, write a PNG.

Stdlib only. Speaks RFB 3.3/3.7/3.8 with security type None (1) only, asks
for 32bpp true-colour Raw encoding and writes the framebuffer as an RGB PNG.

This is deliberately agentless: it is how the smoke tests prove that an image
renders a desktop without relying on anything installed in the guest, which
is exactly the daemon-agnostic path (sandbox -> VNC) the SDK must support.

  rfb_snapshot.py HOST:PORT out.png [--timeout 20] [--pixel X,Y ...]

With --pixel, prints the RGB at each point as `X,Y R G B`.
"""

from __future__ import annotations

import argparse
import socket
import struct
import sys
import zlib


def recv_exact(s: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise ConnectionError(f"connection closed after {len(buf)}/{n} bytes")
        buf += chunk
    return bytes(buf)


def handshake(s: socket.socket) -> tuple[int, int, str]:
    banner = recv_exact(s, 12)
    if not banner.startswith(b"RFB "):
        raise ValueError(f"not an RFB server: {banner!r}")
    major, minor = int(banner[4:7]), int(banner[8:11])
    minor = 8 if (major, minor) >= (3, 8) else (7 if minor >= 7 else 3)
    s.sendall(b"RFB 003.%03d\n" % minor)
    if minor == 3:
        (sec,) = struct.unpack(">I", recv_exact(s, 4))
        if sec != 1:
            raise ValueError(f"server requires security type {sec}; only None is supported")
    else:
        (n,) = struct.unpack("B", recv_exact(s, 1))
        if n == 0:
            (ln,) = struct.unpack(">I", recv_exact(s, 4))
            raise ValueError(recv_exact(s, ln).decode(errors="replace"))
        types = recv_exact(s, n)
        if 1 not in types:
            raise ValueError(f"server security types {list(types)}; only None (1) is supported")
        s.sendall(b"\x01")
        if minor == 8:
            (res,) = struct.unpack(">I", recv_exact(s, 4))
            if res != 0:
                raise ValueError("security handshake failed")
    s.sendall(b"\x01")  # ClientInit: shared
    w, h = struct.unpack(">HH", recv_exact(s, 4))
    recv_exact(s, 16)  # server pixel format (we override it)
    (name_len,) = struct.unpack(">I", recv_exact(s, 4))
    name = recv_exact(s, name_len).decode(errors="replace")
    return w, h, name


def grab(host: str, port: int, timeout: float) -> tuple[int, int, str, bytes]:
    with socket.create_connection((host, port), timeout=timeout) as s:
        s.settimeout(timeout)
        w, h, name = handshake(s)
        # SetPixelFormat: 32bpp, depth 24, little endian, true colour, RGB max 255,
        # shifts R=16 G=8 B=0 -> bytes in memory B,G,R,X.
        pf = struct.pack(">BBBBHHHBBB3x", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
        s.sendall(b"\x00\x00\x00\x00" + pf)
        s.sendall(struct.pack(">BxHi", 2, 1, 0))  # SetEncodings: Raw only
        s.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, w, h))  # full, non-incremental
        fb = bytearray(w * h * 4)
        got = 0
        while got < w * h:
            (msg,) = struct.unpack("B", recv_exact(s, 1))
            if msg == 2:  # Bell
                continue
            if msg == 3:  # ServerCutText
                recv_exact(s, 3)
                (ln,) = struct.unpack(">I", recv_exact(s, 4))
                recv_exact(s, ln)
                continue
            if msg == 1:  # SetColourMapEntries
                recv_exact(s, 1)
                _first, n = struct.unpack(">HH", recv_exact(s, 4))
                recv_exact(s, 6 * n)
                continue
            if msg != 0:
                raise ValueError(f"unexpected server message {msg}")
            recv_exact(s, 1)
            (nrects,) = struct.unpack(">H", recv_exact(s, 2))
            for _ in range(nrects):
                x, y, rw, rh, enc = struct.unpack(">HHHHi", recv_exact(s, 12))
                if enc != 0:
                    raise ValueError(f"server sent encoding {enc}, asked for Raw")
                data = recv_exact(s, rw * rh * 4)
                for row in range(rh):
                    off = ((y + row) * w + x) * 4
                    fb[off : off + rw * 4] = data[row * rw * 4 : (row + 1) * rw * 4]
                got += rw * rh
        return w, h, name, bytes(fb)


def bgrx_to_rgb(fb: bytes, w: int, h: int) -> bytes:
    out = bytearray(w * h * 3)
    out[0::3] = fb[2::4]
    out[1::3] = fb[1::4]
    out[2::3] = fb[0::4]
    return bytes(out)


def write_png(path: str, w: int, h: int, rgb: bytes) -> None:
    raw = b"".join(b"\x00" + rgb[y * w * 3 : (y + 1) * w * 3] for y in range(h))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 6)))
        f.write(chunk(b"IEND", b""))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("addr", help="HOST:PORT")
    ap.add_argument("out", help="output PNG path")
    ap.add_argument("--timeout", type=float, default=20)
    ap.add_argument("--pixel", action="append", default=[], help="X,Y to print")
    a = ap.parse_args()
    host, port = a.addr.rsplit(":", 1)
    w, h, name, fb = grab(host, int(port), a.timeout)
    rgb = bgrx_to_rgb(fb, w, h)
    write_png(a.out, w, h, rgb)
    distinct = len({rgb[i : i + 3] for i in range(0, len(rgb), 3 * 97)})
    print(f"desktop={name!r} size={w}x{h} sampled_colors={distinct} png={a.out}")
    for p in a.pixel:
        x, y = (int(v) for v in p.split(","))
        i = (y * w + x) * 3
        print(f"{x},{y} {rgb[i]} {rgb[i + 1]} {rgb[i + 2]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
