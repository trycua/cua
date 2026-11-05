"""Image facts without third-party libraries: format and pixel size of PNG and JPEG files."""

from __future__ import annotations

import struct
from pathlib import Path


def info(path: Path) -> dict[str, object]:
    """{"format": "png"|"jpeg"|"other", "width": int|None, "height": int|None}"""
    data = path.read_bytes()
    if data[:8] == b"\x89PNG\r\n\x1a\n" and data[12:16] == b"IHDR":
        w, h = struct.unpack(">II", data[16:24])
        return {"format": "png", "width": w, "height": h}
    if data[:2] == b"\xff\xd8":
        i = 2
        while i + 4 <= len(data):
            if data[i] != 0xFF:
                i += 1
                continue
            marker = data[i + 1]
            if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
                i += 2
                continue
            length = struct.unpack(">H", data[i + 2 : i + 4])[0]
            if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
                h, w = struct.unpack(">HH", data[i + 5 : i + 9])
                return {"format": "jpeg", "width": w, "height": h}
            i += 2 + length
        return {"format": "jpeg", "width": None, "height": None}
    return {"format": "other", "width": None, "height": None}
