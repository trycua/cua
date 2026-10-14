#!/usr/bin/env python3
"""Generate and validate the cua-perception synthetic quality corpus."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import tempfile
import zlib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent
CORPUS_VERSION = "1.0.0"
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# A deliberately small, original 5x7 bitmap font keeps generation dependency-free.
FONT = {
    " ": (0, 0, 0, 0, 0, 0, 0),
    "-": (0, 0, 0, 31, 0, 0, 0),
    ".": (0, 0, 0, 0, 0, 12, 12),
    "/": (1, 2, 4, 8, 16, 0, 0),
    ":": (0, 12, 12, 0, 12, 12, 0),
    "?": (14, 17, 1, 2, 4, 0, 4),
    "0": (14, 17, 19, 21, 25, 17, 14),
    "1": (4, 12, 4, 4, 4, 4, 14),
    "2": (14, 17, 1, 2, 4, 8, 31),
    "3": (30, 1, 1, 14, 1, 1, 30),
    "4": (2, 6, 10, 18, 31, 2, 2),
    "5": (31, 16, 16, 30, 1, 1, 30),
    "6": (14, 16, 16, 30, 17, 17, 14),
    "7": (31, 1, 2, 4, 8, 8, 8),
    "8": (14, 17, 17, 14, 17, 17, 14),
    "9": (14, 17, 17, 15, 1, 1, 14),
    "A": (14, 17, 17, 31, 17, 17, 17),
    "B": (30, 17, 17, 30, 17, 17, 30),
    "C": (14, 17, 16, 16, 16, 17, 14),
    "D": (30, 17, 17, 17, 17, 17, 30),
    "E": (31, 16, 16, 30, 16, 16, 31),
    "F": (31, 16, 16, 30, 16, 16, 16),
    "G": (14, 17, 16, 23, 17, 17, 15),
    "H": (17, 17, 17, 31, 17, 17, 17),
    "I": (14, 4, 4, 4, 4, 4, 14),
    "J": (7, 2, 2, 2, 2, 18, 12),
    "K": (17, 18, 20, 24, 20, 18, 17),
    "L": (16, 16, 16, 16, 16, 16, 31),
    "M": (17, 27, 21, 21, 17, 17, 17),
    "N": (17, 25, 21, 19, 17, 17, 17),
    "O": (14, 17, 17, 17, 17, 17, 14),
    "P": (30, 17, 17, 30, 16, 16, 16),
    "Q": (14, 17, 17, 17, 21, 18, 13),
    "R": (30, 17, 17, 30, 20, 18, 17),
    "S": (15, 16, 16, 14, 1, 1, 30),
    "T": (31, 4, 4, 4, 4, 4, 4),
    "U": (17, 17, 17, 17, 17, 17, 14),
    "V": (17, 17, 17, 17, 17, 10, 4),
    "W": (17, 17, 17, 21, 21, 21, 10),
    "X": (17, 17, 10, 4, 10, 17, 17),
    "Y": (17, 17, 10, 4, 4, 4, 4),
    "Z": (31, 1, 2, 4, 8, 16, 31),
}


@dataclass
class Scenario:
    filename: str
    width: int
    height: int
    tags: list[str]
    pixels: bytes
    text: list[dict]
    controls: list[dict]


class Canvas:
    def __init__(self, width: int, height: int, color: tuple[int, int, int]):
        self.width = width
        self.height = height
        self.pixels = bytearray(color * (width * height))

    def point(self, x: int, y: int, color: tuple[int, int, int]) -> None:
        if 0 <= x < self.width and 0 <= y < self.height:
            offset = (y * self.width + x) * 3
            self.pixels[offset : offset + 3] = bytes(color)

    def rect(self, x: int, y: int, width: int, height: int, color: tuple[int, int, int]) -> None:
        x0, y0 = max(0, x), max(0, y)
        x1, y1 = min(self.width, x + width), min(self.height, y + height)
        row = bytes(color) * max(0, x1 - x0)
        for py in range(y0, y1):
            offset = (py * self.width + x0) * 3
            self.pixels[offset : offset + len(row)] = row

    def frame(self, x: int, y: int, width: int, height: int, color: tuple[int, int, int], thickness: int = 1) -> None:
        self.rect(x, y, width, thickness, color)
        self.rect(x, y + height - thickness, width, thickness, color)
        self.rect(x, y, thickness, height, color)
        self.rect(x + width - thickness, y, thickness, height, color)

    def line(self, x0: int, y0: int, x1: int, y1: int, color: tuple[int, int, int], thickness: int = 1) -> None:
        dx, sx = abs(x1 - x0), 1 if x0 < x1 else -1
        dy, sy = -abs(y1 - y0), 1 if y0 < y1 else -1
        error = dx + dy
        while True:
            self.rect(x0 - thickness // 2, y0 - thickness // 2, thickness, thickness, color)
            if x0 == x1 and y0 == y1:
                return
            twice = 2 * error
            if twice >= dy:
                error += dy
                x0 += sx
            if twice <= dx:
                error += dx
                y0 += sy

    def text(self, x: int, y: int, value: str, color: tuple[int, int, int], scale: int = 2) -> list[int]:
        value = value.upper()
        for index, char in enumerate(value):
            glyph = FONT.get(char, FONT["?"])
            gx = x + index * 6 * scale
            for row, bits in enumerate(glyph):
                for column in range(5):
                    if bits & (1 << (4 - column)):
                        self.rect(gx + column * scale, y + row * scale, scale, scale, color)
        width = max(0, len(value) * 6 * scale - scale)
        return [x, y, width, 7 * scale]

    def icon(self, kind: str, x: int, y: int, size: int, color: tuple[int, int, int]) -> None:
        pad = max(2, size // 5)
        if kind == "search":
            self.frame(x + pad, y + pad, size // 2, size // 2, color, max(1, size // 12))
            self.line(x + size // 2, y + size // 2, x + size - pad, y + size - pad, color, max(1, size // 10))
        elif kind == "close":
            self.line(x + pad, y + pad, x + size - pad, y + size - pad, color, max(1, size // 8))
            self.line(x + size - pad, y + pad, x + pad, y + size - pad, color, max(1, size // 8))
        elif kind == "play":
            for row in range(size - 2 * pad):
                width = max(1, row // 2 if row < (size - 2 * pad) // 2 else (size - 2 * pad - row) // 2)
                self.rect(x + pad, y + pad + row, width, 1, color)
        elif kind == "menu":
            for offset in (pad, size // 2, size - pad):
                self.rect(x + pad, y + offset - 1, size - 2 * pad, 3, color)
        elif kind == "gear":
            self.frame(x + pad, y + pad, size - 2 * pad, size - 2 * pad, color, max(1, size // 10))
            self.rect(x + size // 2 - 2, y + size // 2 - 2, 5, 5, color)
        else:
            self.frame(x + pad, y + pad, size - 2 * pad, size - 2 * pad, color, 2)


def text_annotation(identifier: str, value: str, bbox: list[int], role: str) -> dict:
    return {"id": identifier, "text": value, "bbox": bbox, "role": role}


def control_annotation(identifier: str, kind: str, label: str | None, bbox: list[int], state: str) -> dict:
    return {"id": identifier, "kind": kind, "label": label, "bbox": bbox, "state": state}


def native_light() -> Scenario:
    c = Canvas(960, 600, (236, 239, 243))
    c.rect(0, 0, 960, 42, (36, 42, 52))
    title = c.text(18, 13, "SETTINGS", (244, 247, 250), 2)
    c.rect(28, 72, 904, 494, (255, 255, 255))
    c.frame(28, 72, 904, 494, (184, 190, 199), 1)
    heading = c.text(62, 104, "DEVICE SETTINGS", (31, 38, 49), 3)
    label = c.text(64, 178, "DEVICE NAME", (63, 71, 84), 2)
    c.rect(64, 204, 420, 44, (250, 251, 252)); c.frame(64, 204, 420, 44, (130, 139, 151), 2)
    value = c.text(78, 218, "DESK MAC", (28, 35, 45), 2)
    c.rect(64, 282, 22, 22, (43, 119, 226))
    c.line(69, 293, 74, 298, (255, 255, 255), 2); c.line(74, 298, 82, 287, (255, 255, 255), 2)
    check_label = c.text(100, 287, "ENABLE CAPTURE", (40, 47, 58), 2)
    c.rect(64, 342, 420, 8, (218, 222, 228)); c.rect(64, 342, 278, 8, (43, 119, 226))
    c.rect(330, 332, 24, 28, (255, 255, 255)); c.frame(330, 332, 24, 28, (43, 119, 226), 2)
    c.rect(700, 496, 104, 40, (255, 255, 255)); c.frame(700, 496, 104, 40, (103, 112, 125), 2)
    cancel = c.text(716, 510, "CANCEL", (51, 58, 69), 2)
    c.rect(816, 496, 84, 40, (31, 111, 235)); save = c.text(834, 510, "SAVE", (255, 255, 255), 2)
    return Scenario("native-controls-light.png", 960, 600,
                    ["native-controls", "light", "standard-dpi", "small-text"], bytes(c.pixels),
                    [text_annotation("window-title", "SETTINGS", title, "title"), text_annotation("heading", "DEVICE SETTINGS", heading, "heading"),
                     text_annotation("device-label", "DEVICE NAME", label, "label"),
                     text_annotation("device-value", "DESK MAC", value, "value"), text_annotation("capture-label", "ENABLE CAPTURE", check_label, "label"),
                     text_annotation("cancel-text", "CANCEL", cancel, "button-label"), text_annotation("save-text", "SAVE", save, "button-label")],
                    [control_annotation("device-input", "text-field", "DEVICE NAME", [64, 204, 420, 44], "enabled"),
                     control_annotation("capture-checkbox", "checkbox", "ENABLE CAPTURE", [64, 282, 22, 22], "checked"),
                     control_annotation("quality-slider", "slider", None, [64, 332, 290, 28], "enabled"),
                     control_annotation("cancel-button", "button", "CANCEL", [700, 496, 104, 40], "enabled"),
                     control_annotation("save-button", "button", "SAVE", [816, 496, 84, 40], "enabled")])


def native_dark_hidpi() -> Scenario:
    c = Canvas(1440, 900, (24, 27, 33))
    c.rect(0, 0, 1440, 72, (15, 17, 22)); title = c.text(36, 24, "TRANSFER", (245, 247, 250), 4)
    c.rect(72, 126, 1296, 690, (31, 35, 43)); c.frame(72, 126, 1296, 690, (71, 78, 91), 2)
    heading = c.text(120, 174, "COPY FILES", (242, 244, 247), 5)
    source_label = c.text(124, 282, "SOURCE", (173, 181, 194), 3)
    c.rect(120, 324, 900, 72, (21, 24, 30)); c.frame(120, 324, 900, 72, (94, 104, 120), 3)
    source = c.text(146, 346, "/USERS/DEMO/REPORT.PDF", (231, 235, 241), 3)
    c.rect(1050, 324, 180, 72, (55, 62, 74)); browse = c.text(1080, 348, "BROWSE", (248, 249, 251), 3)
    c.rect(120, 474, 48, 48, (52, 132, 242))
    c.line(131, 498, 141, 508, (255, 255, 255), 4); c.line(141, 508, 158, 484, (255, 255, 255), 4)
    replace = c.text(192, 486, "REPLACE EXISTING", (224, 228, 235), 3)
    c.rect(1020, 700, 210, 72, (52, 132, 242)); copy = c.text(1072, 724, "COPY", (255, 255, 255), 3)
    return Scenario("native-controls-dark-hidpi.png", 1440, 900,
                    ["native-controls", "dark", "high-dpi", "small-text"], bytes(c.pixels),
                    [text_annotation("window-title", "TRANSFER", title, "title"), text_annotation("heading", "COPY FILES", heading, "heading"),
                     text_annotation("source-label", "SOURCE", source_label, "label"), text_annotation("source-value", "/USERS/DEMO/REPORT.PDF", source, "value"),
                     text_annotation("browse-text", "BROWSE", browse, "button-label"), text_annotation("replace-label", "REPLACE EXISTING", replace, "label"),
                     text_annotation("copy-text", "COPY", copy, "button-label")],
                    [control_annotation("source-input", "text-field", "SOURCE", [120, 324, 900, 72], "enabled"),
                     control_annotation("browse-button", "button", "BROWSE", [1050, 324, 180, 72], "enabled"),
                     control_annotation("replace-checkbox", "checkbox", "REPLACE EXISTING", [120, 474, 48, 48], "checked"),
                     control_annotation("copy-button", "button", "COPY", [1020, 700, 210, 72], "enabled")])


def browser_canvas() -> Scenario:
    c = Canvas(960, 600, (247, 244, 235))
    c.rect(0, 0, 960, 58, (255, 255, 255)); c.frame(0, 57, 960, 2, (211, 207, 198), 1)
    c.rect(72, 14, 720, 32, (239, 241, 244)); c.frame(72, 14, 720, 32, (200, 204, 211), 1)
    address = c.text(92, 25, "LOCAL DASHBOARD", (67, 72, 80), 1)
    c.rect(32, 88, 896, 472, (255, 253, 247)); c.frame(32, 88, 896, 472, (219, 211, 195), 2)
    heading = c.text(62, 118, "WEEKLY SIGNAL", (40, 38, 34), 3)
    subtitle = c.text(64, 170, "CANVAS RENDER", (111, 103, 91), 1)
    c.line(96, 470, 96, 232, (110, 105, 96), 2); c.line(96, 470, 760, 470, (110, 105, 96), 2)
    points = [(110, 418), (220, 382), (330, 401), (440, 318), (550, 336), (660, 250), (748, 276)]
    for first, second in zip(points, points[1:]): c.line(*first, *second, (217, 77, 55), 5)
    for x, y in points: c.rect(x - 5, y - 5, 11, 11, (217, 77, 55))
    c.rect(772, 188, 116, 54, (36, 119, 166)); export = c.text(790, 208, "EXPORT", (255, 255, 255), 2)
    c.icon("menu", 866, 104, 34, (55, 52, 47))
    return Scenario("browser-canvas-light.png", 960, 600,
                    ["browser-canvas", "light", "standard-dpi", "icon-only"], bytes(c.pixels),
                    [text_annotation("address", "LOCAL DASHBOARD", address, "address"), text_annotation("heading", "WEEKLY SIGNAL", heading, "heading"),
                     text_annotation("subtitle", "CANVAS RENDER", subtitle, "label"),
                     text_annotation("export-text", "EXPORT", export, "button-label")],
                    [control_annotation("chart-canvas", "canvas", None, [80, 210, 700, 280], "enabled"),
                     control_annotation("export-button", "button", "EXPORT", [772, 188, 116, 54], "enabled"),
                     control_annotation("menu-button", "icon-button", None, [866, 104, 34, 34], "enabled")])


def remote_desktop() -> Scenario:
    c = Canvas(960, 600, (12, 46, 63))
    for y in range(600):
        shade = (12 + y // 30, 46 + y // 18, min(100, 63 + y // 12))
        c.rect(0, y, 960, 1, shade)
    c.rect(0, 0, 960, 34, (31, 34, 40)); session = c.text(12, 11, "REMOTE SESSION 04", (224, 228, 234), 1)
    c.rect(0, 566, 960, 34, (25, 28, 34)); c.rect(10, 572, 24, 22, (54, 126, 216))
    c.rect(184, 94, 620, 390, (235, 237, 241)); c.frame(184, 94, 620, 390, (54, 58, 67), 3)
    c.rect(187, 97, 614, 34, (63, 70, 82)); title = c.text(204, 108, "SYSTEM MONITOR", (247, 248, 250), 1)
    c.icon("close", 770, 100, 24, (235, 238, 242))
    c.rect(208, 154, 180, 292, (248, 249, 251)); c.frame(208, 154, 180, 292, (191, 197, 207), 1)
    processes = c.text(222, 174, "PROCESSES", (57, 63, 73), 1)
    network = c.text(222, 202, "NETWORK", (57, 63, 73), 1)
    storage = c.text(222, 230, "STORAGE", (57, 63, 73), 1)
    c.rect(416, 154, 354, 126, (255, 255, 255)); c.frame(416, 154, 354, 126, (191, 197, 207), 1)
    cpu = c.text(434, 170, "CPU 37 PERCENT", (45, 51, 61), 1)
    for x, height in enumerate((34, 48, 43, 68, 51, 72, 62, 84)):
        c.rect(438 + x * 36, 260 - height, 20, height, (53, 133, 203))
    c.rect(416, 300, 354, 146, (255, 255, 255)); c.frame(416, 300, 354, 146, (191, 197, 207), 1)
    status = c.text(434, 320, "STATUS CONNECTED", (40, 112, 70), 1)
    # Deterministic sparse transport noise suggests a compressed remote surface.
    value = 0xC0A52026
    for _ in range(1600):
        value = (1664525 * value + 1013904223) & 0xFFFFFFFF
        x, y = value % 960, (value >> 12) % 600
        base = (value >> 24) & 31
        c.point(x, y, (base + 28, base + 32, base + 38))
    return Scenario("remote-desktop-dark.png", 960, 600,
                    ["remote-desktop-like", "dark", "standard-dpi", "small-text", "noisy"], bytes(c.pixels),
                    [text_annotation("session", "REMOTE SESSION 04", session, "status"), text_annotation("window-title", "SYSTEM MONITOR", title, "title"),
                     text_annotation("processes", "PROCESSES", processes, "list-item"), text_annotation("network", "NETWORK", network, "list-item"),
                     text_annotation("storage", "STORAGE", storage, "list-item"),
                     text_annotation("cpu", "CPU 37 PERCENT", cpu, "value"), text_annotation("status", "STATUS CONNECTED", status, "status")],
                    [control_annotation("remote-window", "window", "SYSTEM MONITOR", [184, 94, 620, 390], "active"),
                     control_annotation("processes-nav", "list-item", "PROCESSES", [214, 164, 160, 24], "selected"),
                     control_annotation("close-window", "icon-button", None, [770, 100, 24, 24], "enabled")])


def downsampled() -> Scenario:
    high = Canvas(960, 600, (244, 247, 250))
    high.rect(40, 40, 880, 520, (255, 255, 255)); high.frame(40, 40, 880, 520, (170, 178, 188), 2)
    high.text(78, 76, "INVENTORY", (30, 38, 48), 4)
    high.rect(78, 142, 804, 52, (226, 232, 240)); high.text(94, 160, "ITEM STATUS OWNER", (64, 73, 85), 2)
    rows = [("ALPHA READY MAY", 220), ("BRAVO HOLD LEE", 288), ("CHARLIE READY KIM", 356), ("DELTA REVIEW SAM", 424)]
    for index, (value, y) in enumerate(rows):
        if index % 2: high.rect(78, y - 12, 804, 48, (247, 249, 251))
        high.text(94, y, value, (42, 49, 59), 2)
    low = downsample_2x(bytes(high.pixels), 960, 600)
    return Scenario("downsampled-table.png", 480, 300,
                    ["downsampled", "light", "small-text", "native-controls"], low,
                    [text_annotation("heading", "INVENTORY", [39, 38, 106, 14], "heading"),
                     text_annotation("header", "ITEM STATUS OWNER", [47, 80, 101, 7], "column-header"),
                     text_annotation("row-alpha", "ALPHA READY MAY", [47, 110, 89, 7], "table-cell"),
                     text_annotation("row-bravo", "BRAVO HOLD LEE", [47, 144, 89, 7], "table-cell"),
                     text_annotation("row-charlie", "CHARLIE READY KIM", [47, 178, 107, 7], "table-cell"),
                     text_annotation("row-delta", "DELTA REVIEW SAM", [47, 212, 101, 7], "table-cell")],
                    [control_annotation("inventory-table", "table", "INVENTORY", [39, 71, 402, 169], "enabled")])


def icon_only() -> Scenario:
    c = Canvas(480, 320, (248, 246, 240))
    c.rect(0, 0, 480, 54, (32, 39, 48))
    specs = [("search", 54), ("play", 142), ("gear", 230), ("menu", 318), ("close", 406)]
    controls = []
    for index, (kind, x) in enumerate(specs):
        c.rect(x - 26, 110, 64, 64, (255, 255, 255)); c.frame(x - 26, 110, 64, 64, (164, 158, 146), 2)
        c.icon(kind, x - 14, 122, 40, (50, 58, 69))
        controls.append(control_annotation(f"tool-{index + 1}", "icon-button", None, [x - 26, 110, 64, 64], "enabled"))
    return Scenario("icon-only-toolbar.png", 480, 320,
                    ["icon-only", "light", "standard-dpi", "native-controls"], bytes(c.pixels), [], controls)


def overlapping() -> Scenario:
    c = Canvas(640, 400, (245, 247, 250))
    c.rect(52, 66, 536, 268, (255, 255, 255)); c.frame(52, 66, 536, 268, (172, 180, 191), 2)
    heading = c.text(82, 98, "FIND COMMAND", (34, 41, 51), 3)
    c.rect(82, 166, 476, 58, (249, 250, 252)); c.frame(82, 166, 476, 58, (92, 105, 123), 2)
    c.icon("search", 94, 175, 40, (64, 76, 92))
    query = c.text(122, 186, "SEARCH FILES", (45, 54, 66), 2)
    c.rect(392, 252, 166, 46, (32, 117, 210)); run = c.text(437, 268, "RUN", (255, 255, 255), 2)
    return Scenario("overlapping-annotations.png", 640, 400,
                    ["overlapping-ocr-icon-boxes", "light", "standard-dpi", "small-text"], bytes(c.pixels),
                    [text_annotation("heading", "FIND COMMAND", heading, "heading"),
                     text_annotation("query", "SEARCH FILES", query, "placeholder"), text_annotation("run-text", "RUN", run, "button-label")],
                    [control_annotation("search-icon", "icon", None, [94, 175, 40, 40], "static"),
                     control_annotation("search-field", "text-field", "SEARCH FILES", [82, 166, 476, 58], "enabled"),
                     control_annotation("run-button", "button", "RUN", [392, 252, 166, 46], "enabled")])


def empty_surface() -> Scenario:
    c = Canvas(640, 400, (250, 250, 248))
    return Scenario("empty-surface.png", 640, 400, ["empty", "light", "standard-dpi"], bytes(c.pixels), [], [])


def noisy_surface() -> Scenario:
    c = Canvas(640, 400, (128, 128, 128))
    value = 0x51A7E123
    for y in range(400):
        for x in range(640):
            value = (1103515245 * value + 12345) & 0x7FFFFFFF
            level = 48 + ((value >> 8) % 160)
            c.point(x, y, (level, (level * 5 + 17) % 192 + 32, (level * 3 + 41) % 192 + 32))
    return Scenario("noisy-surface.png", 640, 400, ["noisy", "dark", "standard-dpi"], bytes(c.pixels), [], [])


def downsample_2x(pixels: bytes, width: int, height: int) -> bytes:
    result = bytearray((width // 2) * (height // 2) * 3)
    for y in range(0, height, 2):
        for x in range(0, width, 2):
            target = ((y // 2) * (width // 2) + x // 2) * 3
            for channel in range(3):
                offsets = [((y + dy) * width + x + dx) * 3 + channel for dy in (0, 1) for dx in (0, 1)]
                result[target + channel] = sum(pixels[offset] for offset in offsets) // 4
    return bytes(result)


def png_chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def encode_png(width: int, height: int, pixels: bytes) -> bytes:
    expected = width * height * 3
    if len(pixels) != expected:
        raise ValueError(f"expected {expected} RGB bytes, got {len(pixels)}")
    scanlines = b"".join(b"\x00" + pixels[y * width * 3 : (y + 1) * width * 3] for y in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return PNG_SIGNATURE + png_chunk(b"IHDR", header) + png_chunk(b"IDAT", zlib.compress(scanlines, 9)) + png_chunk(b"IEND", b"")


def scenarios() -> list[Scenario]:
    return [native_light(), native_dark_hidpi(), browser_canvas(), remote_desktop(), downsampled(), icon_only(), overlapping(), empty_surface(), noisy_surface()]


def manifest_for(items: list[Scenario], pngs: dict[str, bytes]) -> dict:
    return {
        "schema_version": 1,
        "corpus_version": CORPUS_VERSION,
        "license": "MIT",
        "coordinate_system": {"space": "source_pixels", "origin": "top_left", "bbox_format": "x_y_width_height"},
        "metric_definitions": {
            "text_detection_iou": {"unit": "ratio", "description": "Intersection over union between a predicted text box and its matched expected box."},
            "text_recognition_exact": {"unit": "boolean", "description": "Case-sensitive equality between predicted and expected text after trimming outer whitespace."},
            "control_detection_iou": {"unit": "ratio", "description": "Intersection over union between a predicted control box and its matched expected box."},
            "control_kind_accuracy": {"unit": "ratio", "description": "Fraction of matched controls whose predicted kind equals the expected kind."},
            "false_positive_count": {"unit": "count", "description": "Unmatched predictions for an image, including images with no annotations."},
        },
        "images": [
            {
                "file": f"images/{item.filename}",
                "sha256": hashlib.sha256(pngs[item.filename]).hexdigest(),
                "width": item.width,
                "height": item.height,
                "scenario_tags": item.tags,
                "expected": {"text": item.text, "controls": item.controls},
            }
            for item in items
        ],
    }


def write_corpus(destination: Path) -> None:
    image_dir = destination / "images"
    image_dir.mkdir(parents=True, exist_ok=True)
    items = scenarios()
    pngs = {item.filename: encode_png(item.width, item.height, item.pixels) for item in items}
    for filename, data in pngs.items():
        (image_dir / filename).write_bytes(data)
    manifest = manifest_for(items, pngs)
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


def require_exact_keys(value: dict, expected: set[str], context: str) -> None:
    actual = set(value)
    if actual != expected:
        raise ValueError(f"{context} keys differ: expected {sorted(expected)}, got {sorted(actual)}")


def png_dimensions(data: bytes) -> tuple[int, int]:
    if not data.startswith(PNG_SIGNATURE) or data[12:16] != b"IHDR":
        raise ValueError("not a PNG with an IHDR first chunk")
    return struct.unpack(">II", data[16:24])


def validate_manifest(directory: Path) -> None:
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    require_exact_keys(manifest, {"schema_version", "corpus_version", "license", "coordinate_system", "metric_definitions", "images"}, "manifest")
    if manifest["schema_version"] != 1 or manifest["corpus_version"] != CORPUS_VERSION or manifest["license"] != "MIT":
        raise ValueError("unexpected manifest identity")
    require_exact_keys(manifest["coordinate_system"], {"space", "origin", "bbox_format"}, "coordinate_system")
    if manifest["coordinate_system"] != {"space": "source_pixels", "origin": "top_left", "bbox_format": "x_y_width_height"}:
        raise ValueError("coordinate system must describe source-pixel xywh boxes")
    metric_keys = {"text_detection_iou", "text_recognition_exact", "control_detection_iou", "control_kind_accuracy", "false_positive_count"}
    if set(manifest["metric_definitions"]) != metric_keys:
        raise ValueError("metric definitions are not the closed expected set")
    for name, definition in manifest["metric_definitions"].items():
        require_exact_keys(definition, {"unit", "description"}, f"metric {name}")
    if len(manifest["images"]) != 9:
        raise ValueError("corpus must contain exactly nine scenarios")
    required_tags = {"native-controls", "browser-canvas", "remote-desktop-like", "light", "dark", "high-dpi", "downsampled", "small-text", "icon-only", "overlapping-ocr-icon-boxes", "empty", "noisy"}
    observed_tags: set[str] = set()
    observed_files: set[str] = set()
    for index, image in enumerate(manifest["images"]):
        context = f"images[{index}]"
        require_exact_keys(image, {"file", "sha256", "width", "height", "scenario_tags", "expected"}, context)
        require_exact_keys(image["expected"], {"text", "controls"}, f"{context}.expected")
        if image["file"] in observed_files or not image["file"].startswith("images/"):
            raise ValueError(f"duplicate or invalid image path: {image['file']}")
        observed_files.add(image["file"])
        observed_tags.update(image["scenario_tags"])
        path = directory / image["file"]
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != image["sha256"]:
            raise ValueError(f"hash mismatch: {image['file']}")
        if png_dimensions(data) != (image["width"], image["height"]):
            raise ValueError(f"dimension mismatch: {image['file']}")
        annotation_ids: set[str] = set()
        for annotation in image["expected"]["text"]:
            require_exact_keys(annotation, {"id", "text", "bbox", "role"}, f"{context}.text")
            validate_annotation(annotation, image, annotation_ids, context)
            if not annotation["text"]:
                raise ValueError(f"empty expected text in {context}")
        for annotation in image["expected"]["controls"]:
            require_exact_keys(annotation, {"id", "kind", "label", "bbox", "state"}, f"{context}.controls")
            validate_annotation(annotation, image, annotation_ids, context)
            if annotation["label"] is not None and not isinstance(annotation["label"], str):
                raise ValueError(f"invalid control label in {context}")
    missing_tags = required_tags - observed_tags
    if missing_tags:
        raise ValueError(f"missing required scenario tags: {sorted(missing_tags)}")
    actual_pngs = {str(path.relative_to(directory)) for path in (directory / "images").glob("*.png")}
    if actual_pngs != observed_files:
        raise ValueError("manifest image list is not closed over the images directory")


def validate_annotation(annotation: dict, image: dict, ids: set[str], context: str) -> None:
    if not isinstance(annotation["id"], str) or not annotation["id"] or annotation["id"] in ids:
        raise ValueError(f"invalid or duplicate annotation id in {context}")
    ids.add(annotation["id"])
    bbox = annotation["bbox"]
    if not isinstance(bbox, list) or len(bbox) != 4 or any(not isinstance(value, int) for value in bbox):
        raise ValueError(f"invalid bbox in {context}")
    x, y, width, height = bbox
    if x < 0 or y < 0 or width <= 0 or height <= 0 or x + width > image["width"] or y + height > image["height"]:
        raise ValueError(f"out-of-bounds bbox in {context}: {bbox}")


def check_reproducible() -> None:
    validate_manifest(ROOT)
    with tempfile.TemporaryDirectory(prefix="cua-quality-corpus-") as temporary:
        generated = Path(temporary)
        write_corpus(generated)
        validate_manifest(generated)
        expected_files = {"manifest.json"} | {str(path.relative_to(ROOT)) for path in (ROOT / "images").glob("*.png")}
        generated_files = {"manifest.json"} | {str(path.relative_to(generated)) for path in (generated / "images").glob("*.png")}
        if expected_files != generated_files:
            raise ValueError("generated file set differs from checked-in corpus")
        for relative in sorted(expected_files):
            if (ROOT / relative).read_bytes() != (generated / relative).read_bytes():
                raise ValueError(f"generated bytes differ: {relative}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="validate schema and exact reproducibility without changing files")
    args = parser.parse_args()
    if args.check:
        check_reproducible()
        print("quality corpus is valid and reproducible")
    else:
        write_corpus(ROOT)
        validate_manifest(ROOT)
        print("generated quality corpus")


if __name__ == "__main__":
    main()
